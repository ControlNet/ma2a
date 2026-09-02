use std::error::Error;

use iroh::SecretKey;
use ma2a_core::{
    InviteEntropy, MemberCapabilities, RequestId, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_runtime::{EnrollmentAttempt, EnrollmentCreation, Runtime};
use ma2a_store::{
    OwnedSpaceUpdate, RelayConfiguration, RelayTransportConfiguration, Repository, SpaceCreation,
    StoreConfig,
};

use crate::control_sync_e2e_artifacts::{
    AddressFixture, address_records, endpoint_secret, persist_address, persist_relay,
    relay_advertisement,
};

#[path = "support/state.rs"]
mod state;

use state::TempState;
pub(super) use state::clock;

pub(super) type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
const NOW_MS: i64 = 1_700_000_000_000;

pub(super) struct ControlFixture {
    owner_state: TempState,
    candidate_state: TempState,
    pub(super) shared_spaces: [ma2a_core::SpaceId; 2],
    pub(super) private_space: ma2a_core::SpaceId,
    pub(super) owner_secret: SecretKey,
    pub(super) candidate_secret: SecretKey,
    pub(super) owner_records: Vec<ma2a_core::SignedSpaceAddressRecordV1>,
    pub(super) candidate_records: Vec<ma2a_core::SignedSpaceAddressRecordV1>,
    pub(super) owner_advertisements: Vec<ma2a_core::SignedPrivateRelayAdvertisementV1>,
}

impl ControlFixture {
    pub(super) fn owner_config(&self) -> StoreConfig {
        StoreConfig::new(&self.owner_state.0)
    }

    pub(super) fn candidate_config(&self) -> StoreConfig {
        StoreConfig::new(&self.candidate_state.0)
    }
}

pub(super) async fn control_fixture() -> TestResultValue<ControlFixture> {
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
    let (shared_spaces, private_space) = create_and_enroll(&owner_state, &candidate_state).await?;
    let (owner_secret, initial_owner_records, owner_records, owner_advertisements) =
        prepare_owner(&owner_state, shared_spaces)?;
    let (candidate_secret, candidate_records) =
        prepare_candidate(&candidate_state, shared_spaces, &initial_owner_records)?;
    Ok(ControlFixture {
        owner_state,
        candidate_state,
        shared_spaces,
        private_space,
        owner_secret,
        candidate_secret,
        owner_records,
        candidate_records,
        owner_advertisements,
    })
}

async fn create_and_enroll(
    owner_state: &TempState,
    candidate_state: &TempState,
) -> TestResultValue<([ma2a_core::SpaceId; 2], ma2a_core::SpaceId)> {
    let owner_config = StoreConfig::new(&owner_state.0);
    let candidate_config = StoreConfig::new(&candidate_state.0);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_status = owner.handle().status().await?;
    owner.shutdown().await?;
    let owner_member = SpaceMemberV1::new(
        owner_status.endpoint_id(),
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let mut repository = Repository::open(&owner_config)?;
    let first = repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS)?,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let second = repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS + 1)?,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let private = repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS + 2)?,
        owner_member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let shared = [first.space_id(), second.space_id()];
    let private = private.space_id();
    drop(repository);
    let owner = Runtime::start_with_clock(owner_config, clock()).await?;
    let candidate = Runtime::start_with_clock(candidate_config, clock()).await?;
    enroll_spaces(&owner, &candidate, shared).await?;
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok((shared, private))
}

type OwnerArtifacts = (
    SecretKey,
    Vec<ma2a_core::SignedSpaceAddressRecordV1>,
    Vec<ma2a_core::SignedSpaceAddressRecordV1>,
    Vec<ma2a_core::SignedPrivateRelayAdvertisementV1>,
);

fn prepare_owner(
    state: &TempState,
    spaces: [ma2a_core::SpaceId; 2],
) -> TestResultValue<OwnerArtifacts> {
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    for (offset, space_id) in spaces.into_iter().enumerate() {
        let chain = repository
            .load_space_chain(space_id)?
            .ok_or("owner chain missing")?;
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            u64::try_from(NOW_MS + 10 + i64::try_from(offset)?)?,
            SpaceManifestMembership::new(chain.members().to_vec(), vec![]),
        ))?;
    }
    let port = repository
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let secret = endpoint_secret(&state.0)?;
    let initial_sequence = next_address_sequence(&repository, spaces, secret.public().into())?;
    let advanced_sequence = initial_sequence
        .checked_add(1)
        .ok_or("owner address sequence exhausted")?;
    let initial = address_records(
        spaces,
        &secret,
        AddressFixture {
            port,
            sequence: initial_sequence,
        },
    )?;
    let advanced = address_records(
        spaces,
        &secret,
        AddressFixture {
            port,
            sequence: advanced_sequence,
        },
    )?;
    let advertisements = spaces
        .into_iter()
        .map(|space_id| relay_advertisement(space_id, &secret))
        .collect::<TestResultValue<Vec<_>>>()?;
    for record in initial.iter().chain(&advanced) {
        persist_address(&mut repository, record, u64::try_from(NOW_MS)?)?;
    }
    for advertisement in &advertisements {
        persist_relay(&mut repository, advertisement, u64::try_from(NOW_MS)?)?;
    }
    repository.set_relay_configuration(&RelayConfiguration {
        public_fallback_enabled: false,
        public_relay_urls: Vec::new(),
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:0".to_owned()),
        private_relay_url: Some("https://relay.example.invalid".to_owned()),
        served_spaces: spaces.to_vec(),
        transport: Some(RelayTransportConfiguration::ExternalTlsTermination),
    })?;
    Ok((secret, initial, advanced, advertisements))
}

fn prepare_candidate(
    state: &TempState,
    spaces: [ma2a_core::SpaceId; 2],
    owner_records: &[ma2a_core::SignedSpaceAddressRecordV1],
) -> TestResultValue<(SecretKey, Vec<ma2a_core::SignedSpaceAddressRecordV1>)> {
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    for record in owner_records {
        persist_address(&mut repository, record, u64::try_from(NOW_MS)?)?;
    }
    let port = repository
        .endpoint_bind_port()?
        .ok_or("candidate bind port missing")?;
    let secret = endpoint_secret(&state.0)?;
    let sequence = next_address_sequence(&repository, spaces, secret.public().into())?;
    let records = address_records(spaces, &secret, AddressFixture { port, sequence })?;
    for record in &records {
        persist_address(&mut repository, record, u64::try_from(NOW_MS)?)?;
    }
    Ok((secret, records))
}

fn next_address_sequence(
    repository: &Repository,
    spaces: [ma2a_core::SpaceId; 2],
    endpoint_id: ma2a_core::EndpointId,
) -> TestResultValue<u64> {
    let mut next = 0;
    for space_id in spaces {
        if let Some(record) = repository.address_record(space_id, endpoint_id)? {
            next = next.max(
                record
                    .sequence()
                    .checked_add(1)
                    .ok_or("address sequence exhausted")?,
            );
        }
    }
    Ok(next)
}

async fn enroll_spaces(
    owner: &Runtime,
    candidate: &Runtime,
    spaces: [ma2a_core::SpaceId; 2],
) -> TestResult {
    for (offset, space_id) in spaces.into_iter().enumerate() {
        let seed = u8::try_from(offset)?;
        let ticket = owner
            .handle()
            .create_enrollment_invite(EnrollmentCreation::new(
                space_id,
                300_000,
                InviteEntropy::from_bytes([0x31 + seed; 16], [0x41 + seed; 32]),
            )?)
            .await?;
        candidate
            .handle()
            .redeem_enrollment(EnrollmentAttempt::new(
                ticket,
                RequestId::try_from([0x51 + seed; 16].as_slice())?,
                "candidate".to_owned(),
            ))
            .await?;
    }
    Ok(())
}
