//! An authority remembers which generation answered each departure request with a removal, so a
//! retry of that request is answered the same way, even after the member joined again.
#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    EndpointId, MemberCapabilities, RequestId, SpaceAuthoritySecret, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{DepartureRequest, OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use support::{TempState, TestResult};

fn member(seed: u8, owner: bool) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let key = SpaceAuthoritySecret::from_bytes([seed; 32]).public_key();
    SpaceMemberV1::new(
        EndpointId::try_from(key.as_bytes().as_slice())?,
        format!("member-{seed}"),
        MemberCapabilities::new(true, owner),
    )
}

fn sorted(mut members: Vec<SpaceMemberV1>) -> Vec<SpaceMemberV1> {
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    members
}

#[test]
fn a_removal_answering_a_departure_records_its_generation_with_the_commit() -> TestResult {
    // Given: an owned Space the peer joined.
    let state = TempState::new("departure-receipts")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let (owner, peer) = (member(1, true)?, member(2, false)?);
    let space_id = repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let joined = SpaceManifestMembership::new(sorted(vec![owner.clone(), peer.clone()]), vec![]);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined.clone()))?;
    let request = DepartureRequest::new(
        peer.endpoint_id(),
        RequestId::try_from([0x51; 16].as_slice())?,
    );
    let other = DepartureRequest::new(
        peer.endpoint_id(),
        RequestId::try_from([0x52; 16].as_slice())?,
    );
    assert_eq!(repository.departure_receipt(space_id, request)?, None);

    // When: the peer's departure request removes it.
    let removal = SpaceManifestMembership::new(
        vec![owner],
        vec![SpaceRevocationV1::new(peer.endpoint_id())],
    );
    let removed = repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 3_000, removal).answering(request))?;

    // Then: that request, and only it, names the removing generation, which outlives a rejoin.
    let generation = removed.chain().latest_generation();
    assert_eq!(
        repository.departure_receipt(space_id, request)?,
        Some(generation)
    );
    assert_eq!(repository.departure_receipt(space_id, other)?, None);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(space_id, 4_000, joined))?;
    assert_eq!(
        repository.departure_receipt(space_id, request)?,
        Some(generation)
    );
    Ok(())
}

#[test]
fn a_receipt_is_refused_for_a_generation_that_does_not_remove_its_sender() -> TestResult {
    // Given: an owned Space the peer joined, and a departure request from the peer.
    let state = TempState::new("departure-receipts-refused")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let (owner, peer, other) = (member(1, true)?, member(2, false)?, member(3, false)?);
    let space_id = repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let joined = SpaceManifestMembership::new(sorted(vec![owner.clone(), peer.clone()]), vec![]);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined))?;
    let request = DepartureRequest::new(
        peer.endpoint_id(),
        RequestId::try_from([0x53; 16].as_slice())?,
    );
    let revision = repository.revision()?;

    // When / Then: a change that keeps the peer, and one that only admits another member, are
    // not its answer, and neither commits anything.
    let kept = SpaceManifestMembership::new(
        sorted(vec![owner.clone(), peer.clone(), other.clone()]),
        vec![],
    );
    assert!(
        repository
            .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 3_000, kept).answering(request))
            .is_err()
    );
    let revoked_other = SpaceManifestMembership::new(
        sorted(vec![owner, peer]),
        vec![SpaceRevocationV1::new(other.endpoint_id())],
    );
    assert!(
        repository
            .advance_owned_space(
                &OwnedSpaceUpdate::new(space_id, 3_000, revoked_other).answering(request)
            )
            .is_err()
    );
    assert_eq!(repository.revision()?, revision);
    assert_eq!(repository.departure_receipt(space_id, request)?, None);
    Ok(())
}

#[test]
fn a_late_departure_is_bound_only_to_a_generation_that_removed_its_sender() -> TestResult {
    // Given: the owner admitted the peer (generation 1) and then removed it (generation 2).
    let state = TempState::new("departure-receipts-bound")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let (owner, peer) = (member(1, true)?, member(2, false)?);
    let space_id = repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let joined = SpaceManifestMembership::new(sorted(vec![owner.clone(), peer.clone()]), vec![]);
    let admitted = repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined))?
        .chain()
        .latest_generation();
    let removal = SpaceManifestMembership::new(
        vec![owner],
        vec![SpaceRevocationV1::new(peer.endpoint_id())],
    );
    let removed = repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 3_000, removal))?
        .chain()
        .latest_generation();
    let request = DepartureRequest::new(
        peer.endpoint_id(),
        RequestId::try_from([0x54; 16].as_slice())?,
    );
    let revision = repository.revision()?;

    // When / Then: the admitting generation, or one the chain does not hold, is refused.
    assert!(
        repository
            .bind_departure_receipt((space_id, admitted), request)
            .is_err()
    );
    assert!(
        repository
            .bind_departure_receipt((space_id, removed + 1), request)
            .is_err()
    );
    assert_eq!(repository.departure_receipt(space_id, request)?, None);

    // And: the removing generation is bound, once, without changing the revision.
    repository.bind_departure_receipt((space_id, removed), request)?;
    assert_eq!(
        repository.departure_receipt(space_id, request)?,
        Some(removed)
    );
    assert!(
        repository
            .bind_departure_receipt((space_id, removed), request)
            .is_err()
    );
    assert_eq!(repository.revision()?, revision);
    Ok(())
}
