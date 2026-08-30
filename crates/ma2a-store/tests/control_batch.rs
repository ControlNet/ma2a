//! Atomic control-batch persistence coverage.

#[path = "control_batch/manifests.rs"]
mod manifests;
#[path = "common/support.rs"]
mod support;

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, MemberCapabilities,
    PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
    SpaceAddressRecordV1, SpaceAuthorizationView, SpaceGenesisIdentity, SpaceGenesisOwner,
    SpaceGenesisV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, ControlBatch, Repository, StoreConfig,
    ValidatedAddressRecord, ValidatedRelayAdvertisement,
};
use support::{TempState, TestResult};

const NOW_MS: u64 = 15;

struct Fixture {
    _state: TempState,
    repository: Repository,
    chain: ma2a_core::SpaceChain,
    endpoint_secret: SecretKey,
}

impl Fixture {
    fn new(label: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let state = TempState::new(label)?;
        let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
        let authority = ma2a_core::SpaceAuthoritySecret::from_bytes([0x41; 32]);
        let endpoint_secret = SecretKey::from_bytes(&[0x43; 32]);
        let endpoint_id = endpoint_secret.public().into();
        let member = SpaceMemberV1::new(
            endpoint_id,
            "store-fixture-owner".to_owned(),
            MemberCapabilities::new(true, true),
        )?;
        let genesis = SpaceGenesisV1::new(
            SpaceGenesisIdentity::new([0x42; 32], 1, authority.public_key())?,
            SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
        )
        .sign(&authority)?;
        let chain = ma2a_core::SpaceChain::from_genesis(genesis)?;
        repository.persist_space_chain(&chain)?;
        Ok(Self {
            _state: state,
            repository,
            chain,
            endpoint_secret,
        })
    }

    fn address(
        &self,
        sequence: u64,
        port: u16,
    ) -> Result<ValidatedAddressRecord, Box<dyn std::error::Error + Send + Sync>> {
        let endpoint_id = self.endpoint_secret.public().into();
        let signed = SpaceAddressRecordV1::new(
            AddressRecordScope::new(self.chain.space_id(), endpoint_id),
            AddressRecordValidity::new(sequence, 10, 20)?,
            AddressEndpointDataV1::new(vec![TransportAddr::Ip(([127, 0, 0, 1], port).into())])?,
        )
        .sign(&self.endpoint_secret)?;
        let authorization = SpaceAuthorizationView::from_chain(&self.chain);
        Ok(ValidatedAddressRecord::parse(
            signed.canonical_bytes(),
            AddressRecordValidation::new(
                AddressRecordTarget::new(self.chain.space_id(), endpoint_id),
                &authorization,
                NOW_MS,
            ),
        )?)
    }

    fn relay(
        &self,
        sequence: u64,
        suffix: u16,
    ) -> Result<ValidatedRelayAdvertisement, Box<dyn std::error::Error + Send + Sync>> {
        let signed = PrivateRelayAdvertisementV1::new(
            PrivateRelayAdvertisementScope::new(
                self.chain.space_id(),
                self.endpoint_secret.public().into(),
            ),
            format!("https://relay-{suffix}.example.invalid").parse()?,
            PrivateRelayAdvertisementValidity::new(sequence, 10, 20)?,
        )?
        .sign(&self.endpoint_secret)?;
        ValidatedRelayAdvertisement::parse(
            signed.canonical_bytes(),
            &SpaceAuthorizationView::from_chain(&self.chain),
            NOW_MS,
        )
        .map_err(|_| "relay advertisement validation failed".into())
    }
}

#[test]
fn control_batch_commits_all_advances_at_one_revision() -> TestResult {
    // Given
    let mut fixture = Fixture::new("control-batch")?;
    let address = fixture.address(1, 4101)?;
    let relay = fixture.relay(1, 1)?;
    let revision = fixture.repository.revision()?;

    // When
    let committed = fixture
        .repository
        .persist_control_batch(&ControlBatch::new(Vec::new(), vec![address], vec![relay]))?;

    // Then
    assert_eq!(committed, Some(revision + 1));
    Ok(())
}

#[test]
fn control_batch_rejects_address_rollback_and_fork_atomically() -> TestResult {
    for (label, rejected) in [("rollback", (1, 4101)), ("fork", (2, 4102))] {
        // Given
        let mut fixture = Fixture::new(label)?;
        let accepted = fixture.address(2, 4101)?;
        fixture
            .repository
            .persist_control_batch(&ControlBatch::new(Vec::new(), vec![accepted], Vec::new()))?;
        let revision = fixture.repository.revision()?;
        let advance = fixture.address(3, 4103)?;
        let conflict = fixture.address(rejected.0, rejected.1)?;

        // When
        let result = fixture.repository.persist_control_batch(&ControlBatch::new(
            Vec::new(),
            vec![advance, conflict],
            Vec::new(),
        ));

        // Then
        assert!(result.is_err());
        assert_eq!(fixture.repository.revision()?, revision);
        assert_eq!(
            fixture
                .repository
                .address_record(
                    fixture.chain.space_id(),
                    fixture.endpoint_secret.public().into(),
                )?
                .ok_or("address missing")?
                .sequence(),
            2
        );
    }
    Ok(())
}

#[test]
fn control_batch_rejects_relay_rollback_and_fork_atomically() -> TestResult {
    for (label, rejected) in [("relay-rollback", (1, 1)), ("relay-fork", (2, 2))] {
        // Given
        let mut fixture = Fixture::new(label)?;
        let accepted = fixture.relay(2, 1)?;
        fixture
            .repository
            .persist_control_batch(&ControlBatch::new(Vec::new(), Vec::new(), vec![accepted]))?;
        let revision = fixture.repository.revision()?;
        let advance = fixture.relay(3, 3)?;
        let conflict = fixture.relay(rejected.0, rejected.1)?;

        // When
        let result = fixture.repository.persist_control_batch(&ControlBatch::new(
            Vec::new(),
            Vec::new(),
            vec![advance, conflict],
        ));

        // Then
        assert!(result.is_err());
        assert_eq!(fixture.repository.revision()?, revision);
        assert_eq!(
            fixture
                .repository
                .relay_advertisement(
                    fixture.chain.space_id(),
                    fixture.endpoint_secret.public().into(),
                )?
                .ok_or("relay missing")?
                .sequence(),
            2
        );
    }
    Ok(())
}

#[test]
fn control_batch_identical_replay_is_idempotent() -> TestResult {
    // Given
    let mut fixture = Fixture::new("replay")?;
    let address = fixture.address(2, 4101)?;
    let relay = fixture.relay(2, 1)?;
    fixture
        .repository
        .persist_control_batch(&ControlBatch::new(
            Vec::new(),
            vec![address.clone()],
            vec![relay.clone()],
        ))?;
    let revision = fixture.repository.revision()?;

    // When
    let replayed = fixture
        .repository
        .persist_control_batch(&ControlBatch::new(Vec::new(), vec![address], vec![relay]))?;

    // Then
    assert_eq!(replayed, None);
    assert_eq!(fixture.repository.revision()?, revision);
    Ok(())
}
