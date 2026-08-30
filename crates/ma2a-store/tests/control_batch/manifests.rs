use ma2a_core::{
    SpaceAuthoritySecret, SpaceChain, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
};
use ma2a_store::ControlBatch;

use super::{Fixture, TestResult};

fn advance(chain: &SpaceChain, issued_at_ms: u64) -> Result<SpaceChain, ma2a_store::StoreError> {
    let authority = SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let mut candidate = chain.clone();
    let manifest = SpaceManifestV1::new(
        SpaceManifestLink::new(
            candidate.space_id(),
            candidate.latest_generation() + 1,
            candidate.latest_hash(),
        ),
        issued_at_ms,
        SpaceManifestMembership::new(
            candidate.members().to_vec(),
            candidate.revocations().to_vec(),
        ),
    )?
    .sign(&authority)?;
    candidate.apply(&manifest)?;
    Ok(candidate)
}

fn chain_through_two(
    fixture: &Fixture,
) -> Result<(SpaceChain, SpaceChain), ma2a_store::StoreError> {
    let first = advance(&fixture.chain, 100)?;
    let second = advance(&first, 101)?;
    Ok((first, second))
}

#[test]
fn control_batch_rejects_manifest_rollback_without_changing_revision() -> TestResult {
    // Given
    let mut fixture = Fixture::new("manifest-rollback")?;
    let (first, second) = chain_through_two(&fixture)?;
    fixture
        .repository
        .persist_control_batch(&ControlBatch::new(
            vec![second.clone()],
            Vec::new(),
            Vec::new(),
        ))?;
    let revision = fixture.repository.revision()?;

    // When
    let result = fixture.repository.persist_control_batch(&ControlBatch::new(
        vec![first],
        Vec::new(),
        Vec::new(),
    ));

    // Then
    assert!(result.is_err());
    assert_eq!(fixture.repository.revision()?, revision);
    assert_eq!(
        fixture
            .repository
            .load_space_chain(second.space_id())?
            .ok_or("chain missing")?,
        second
    );
    Ok(())
}

#[test]
fn control_batch_rejects_same_generation_manifest_fork() -> TestResult {
    // Given
    let mut fixture = Fixture::new("manifest-fork")?;
    let (first, accepted) = chain_through_two(&fixture)?;
    let fork = advance(&first, 102)?;
    fixture
        .repository
        .persist_control_batch(&ControlBatch::new(
            vec![accepted.clone()],
            Vec::new(),
            Vec::new(),
        ))?;
    let revision = fixture.repository.revision()?;

    // When
    let result = fixture.repository.persist_control_batch(&ControlBatch::new(
        vec![fork],
        Vec::new(),
        Vec::new(),
    ));

    // Then
    assert!(result.is_err());
    assert_eq!(fixture.repository.revision()?, revision);
    assert_eq!(
        fixture
            .repository
            .load_space_chain(accepted.space_id())?
            .ok_or("chain missing")?,
        accepted
    );
    Ok(())
}

#[test]
fn control_batch_identical_manifest_replay_is_idempotent() -> TestResult {
    // Given
    let mut fixture = Fixture::new("manifest-replay")?;
    let (_, accepted) = chain_through_two(&fixture)?;
    fixture
        .repository
        .persist_control_batch(&ControlBatch::new(
            vec![accepted.clone()],
            Vec::new(),
            Vec::new(),
        ))?;
    let revision = fixture.repository.revision()?;

    // When
    let replayed = fixture
        .repository
        .persist_control_batch(&ControlBatch::new(vec![accepted], Vec::new(), Vec::new()))?;

    // Then
    assert_eq!(replayed, None);
    assert_eq!(fixture.repository.revision()?, revision);
    Ok(())
}

#[test]
fn invalid_manifest_rolls_back_valid_address_and_relay_advances() -> TestResult {
    // Given
    let mut fixture = Fixture::new("manifest-mixed-atomicity")?;
    let (stale, accepted) = chain_through_two(&fixture)?;
    let address = fixture.address(2, 4102)?;
    let relay = fixture.relay(2, 2)?;
    fixture
        .repository
        .persist_control_batch(&ControlBatch::new(
            vec![accepted.clone()],
            vec![address],
            vec![relay],
        ))?;
    let revision = fixture.repository.revision()?;
    let endpoint_id = fixture.endpoint_secret.public().into();

    // When
    let result = fixture.repository.persist_control_batch(&ControlBatch::new(
        vec![stale],
        vec![fixture.address(3, 4103)?],
        vec![fixture.relay(3, 3)?],
    ));

    // Then
    assert!(result.is_err());
    assert_eq!(fixture.repository.revision()?, revision);
    let stored_chain = fixture
        .repository
        .load_space_chain(accepted.space_id())?
        .ok_or("chain missing")?;
    assert_eq!(
        stored_chain.latest_generation(),
        accepted.latest_generation()
    );
    assert_eq!(stored_chain.latest_hash(), accepted.latest_hash());
    assert_eq!(
        fixture
            .repository
            .address_record(accepted.space_id(), endpoint_id)?
            .ok_or("address missing")?
            .sequence(),
        2
    );
    assert_eq!(
        fixture
            .repository
            .relay_advertisement(accepted.space_id(), endpoint_id)?
            .ok_or("relay missing")?
            .sequence(),
        2
    );
    Ok(())
}
