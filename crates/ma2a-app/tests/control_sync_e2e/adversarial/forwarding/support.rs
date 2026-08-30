use std::{net::Ipv4Addr, sync::Arc};

use iroh::TransportAddr;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlPageV1, ControlResponseV1, SpaceAddressRecordV1,
    SpaceAuthoritySecret, SpaceChain,
};
use ma2a_net::{
    AddressLookupClock, EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_runtime::Runtime;
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, ControlSpaceState, KeyKind, KeyReference,
    KeyStore, Repository, StoreConfig, ValidatedAddressRecord,
};
use tokio::sync::mpsc;

use super::super::super::control_sync_e2e_support::{ControlFixture, TestResult, clock};

const NOW_MS: u64 = 1_700_000_000_000;

#[derive(Debug)]
struct FixedClock;

impl AddressLookupClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DurableState {
    revision: u64,
    control_spaces: Vec<ControlSpaceState>,
    private_chain: Option<SpaceChain>,
    lookup_addresses: Vec<String>,
}

pub(super) struct AddressArtifact {
    pub(super) space_id: ma2a_core::SpaceId,
    pub(super) sequence: u64,
    pub(super) port: u16,
    pub(super) issued_at_ms: u64,
    pub(super) expires_at_ms: u64,
}

pub(super) async fn reject_page(fixture: ControlFixture, page: ControlPageV1) -> TestResult {
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let owner_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner port missing")?;
    let response = ControlResponseV1::new(vec![page])?.encode()?;
    let (enrollment_calls, _enrollment_receiver) = mpsc::channel(1);
    let (control_calls, mut control_receiver) = mpsc::channel(1);
    let malicious_owner = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&fixture.owner_secret.to_bytes())?,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(enrollment_calls, Some(owner_port))
            .with_control(control_calls, true),
    )
    .await?;
    let responder = tokio::spawn(async move {
        let call = control_receiver
            .recv()
            .await
            .ok_or("control call missing")?;
        call.respond(Ok(response));
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;
    let before = durable_state(&candidate_config, &fixture)?;

    let result = candidate.handle().sync_control().await;

    assert!(result.is_err());
    responder.await??;
    assert_eq!(durable_state(&candidate_config, &fixture)?, before);
    candidate.shutdown().await?;
    malicious_owner.shutdown().await?;
    Ok(())
}

fn durable_state(
    config: &StoreConfig,
    fixture: &ControlFixture,
) -> Result<DurableState, Box<dyn std::error::Error + Send + Sync>> {
    let repository = Repository::open(config)?;
    let candidate_id = fixture.candidate_secret.public().into();
    let control_spaces = repository.control_spaces_for(candidate_id)?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(
        control_spaces
            .iter()
            .map(ControlSpaceState::authorization)
            .collect(),
    )?;
    for state in &control_spaces {
        let authorization = state.authorization();
        for record in state.address_records() {
            lookup.cache(ValidatedAddressRecord::parse(
                record.signed_record(),
                AddressRecordValidation::new(
                    AddressRecordTarget::new(record.space_id(), record.endpoint_id()),
                    &authorization,
                    NOW_MS,
                ),
            )?)?;
        }
    }
    let lookup_addresses = lookup
        .resolve_endpoint(fixture.owner_secret.public())
        .map(|resolved| resolved.data.addrs().map(ToString::to_string).collect())
        .unwrap_or_default();
    Ok(DurableState {
        revision: repository.revision()?,
        control_spaces,
        private_chain: repository.load_space_chain(fixture.private_space)?,
        lookup_addresses,
    })
}

pub(super) fn artifact_page(
    space_id: ma2a_core::SpaceId,
    kind: ControlArtifactKind,
    bytes: &[u8],
) -> Result<ControlPageV1, ma2a_core::ProtocolError> {
    ControlPageV1::new(
        space_id,
        false,
        vec![ControlArtifactV1::new(kind, bytes.to_vec())?],
    )
}

pub(super) fn address_record(
    fixture: &ControlFixture,
    artifact: &AddressArtifact,
) -> Result<ma2a_core::SignedSpaceAddressRecordV1, ma2a_core::ProtocolError> {
    SpaceAddressRecordV1::new(
        AddressRecordScope::new(artifact.space_id, fixture.owner_secret.public().into()),
        AddressRecordValidity::new(
            artifact.sequence,
            artifact.issued_at_ms,
            artifact.expires_at_ms,
        )?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(
            (Ipv4Addr::LOCALHOST, artifact.port).into(),
        )])?,
    )
    .sign(&fixture.owner_secret)
}

pub(super) fn authority_secret(
    config: &StoreConfig,
    space_id: ma2a_core::SpaceId,
) -> Result<SpaceAuthoritySecret, Box<dyn std::error::Error + Send + Sync>> {
    let mut reference = String::from("space-");
    for byte in space_id.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut reference, "{byte:02x}")?;
    }
    let protected = KeyStore::open(config.state_dir())?
        .read(KeyKind::SpaceAuthority, &KeyReference::parse(&reference)?)?;
    Ok(SpaceAuthoritySecret::try_from_bytes(protected.as_bytes())?)
}
