//! Atomic control-batch persistence coverage.

#[path = "common/support.rs"]
mod support;

use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, ControlBatch, Repository, StoreConfig,
    ValidatedAddressRecord,
};
use support::{TempState, TestResult};

#[test]
fn control_batch_commits_all_advances_at_one_revision() -> TestResult {
    // Given
    let state = TempState::new("control-batch")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let authority = ma2a_core::SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let endpoint_secret = ma2a_core::SpaceAuthoritySecret::from_bytes([0x43; 32]);
    let endpoint_id =
        ma2a_core::EndpointId::try_from(endpoint_secret.public_key().as_bytes().as_slice())?;
    let member = ma2a_core::SpaceMemberV1::new(
        endpoint_id,
        "store-fixture-owner".to_owned(),
        ma2a_core::MemberCapabilities::new(true, true),
    )?;
    let genesis = ma2a_core::SpaceGenesisV1::new(
        ma2a_core::SpaceGenesisIdentity::new([0x42; 32], 1, authority.public_key())?,
        ma2a_core::SpaceGenesisOwner::new(member, ma2a_core::SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let chain = ma2a_core::SpaceChain::from_genesis(genesis.clone())?;
    let authorization = ma2a_core::SpaceAuthorizationView::from_chain(&chain);
    let record = ma2a_core::SpaceAddressRecordV1::new(
        ma2a_core::AddressRecordScope::new(genesis.space_id(), endpoint_id),
        ma2a_core::AddressRecordValidity::new(1, 10, 20)?,
        ma2a_core::AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(&iroh_base::SecretKey::from_bytes(&[0x43; 32]))?;
    let validated = ValidatedAddressRecord::parse(
        record.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(genesis.space_id(), endpoint_id),
            &authorization,
            15,
        ),
    )?;

    // When
    let revision = repository.persist_control_batch(&ControlBatch::new(
        vec![chain],
        vec![validated],
        Vec::new(),
    ))?;

    // Then
    assert_eq!(revision, Some(1));
    assert!(repository.load_space_chain(genesis.space_id())?.is_some());
    assert_eq!(
        repository
            .address_record(genesis.space_id(), endpoint_id)?
            .ok_or("batched address missing")?
            .sequence(),
        1
    );
    assert_eq!(repository.revision()?, 1);
    Ok(())
}
