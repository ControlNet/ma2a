use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{
    EndpointId, MemberCapabilities, RequestId, SpaceAuthoritySecret, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{OwnedSpaceUpdate, SpaceCreation, StoreConfig};

use super::OwnedMemberRevocation;
use crate::store::StoreBackend;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

/// A private state directory removed on drop.
struct State(PathBuf);

impl State {
    fn new() -> std::io::Result<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-departure-receipt-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_retried_departure_never_removes_a_member_who_joined_again() -> TestResult {
    // Given: a Space this Runtime owns, which the peer joined.
    let state = State::new()?;
    let mut backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
    let local = backend.initialize()?.endpoint_id;
    let owner = SpaceMemberV1::new(
        local,
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let peer_key = SpaceAuthoritySecret::from_bytes([0x42; 32]).public_key();
    let peer = EndpointId::try_from(peer_key.as_bytes().as_slice())?;
    let space_id = backend
        .repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members = vec![
        owner,
        SpaceMemberV1::new(
            peer,
            "peer".to_owned(),
            MemberCapabilities::new(true, false),
        )?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let joined = SpaceManifestMembership::new(members, Vec::new());
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined.clone()))?;
    let departure = OwnedMemberRevocation {
        space_id,
        endpoint_id: peer,
        issued_at_ms: 3_000,
        local_endpoint_id: local,
        request_id: Some(RequestId::try_from([0x61; 16].as_slice())?),
    };

    // And: its departure request removed it, and it joined again through a new invitation.
    let first = backend.revoke_owned_space_member(departure)?;
    assert!(!first.replayed);
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 4_000, joined))?;
    let rejoined = backend
        .repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    let revision = backend.repository.revision()?;

    // When: the first departure request arrives again.
    let retried = backend.revoke_owned_space_member(departure)?;

    // Then: it gets the first answer, and the second membership, chain head, and revision stay.
    assert!(retried.replayed);
    assert_eq!(retried.chain, first.chain);
    let after = backend
        .repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    assert_eq!(after, rejoined);
    assert!(
        after
            .members()
            .iter()
            .any(|member| member.endpoint_id() == peer)
    );
    assert_eq!(backend.repository.revision()?, revision);
    Ok(())
}

#[test]
fn each_departure_of_a_member_who_left_twice_gets_its_own_answer() -> TestResult {
    // Given: a Space this Runtime owns, which the peer joined.
    let state = State::new()?;
    let mut backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
    let local = backend.initialize()?.endpoint_id;
    let owner = SpaceMemberV1::new(
        local,
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let peer_key = SpaceAuthoritySecret::from_bytes([0x43; 32]).public_key();
    let peer = EndpointId::try_from(peer_key.as_bytes().as_slice())?;
    let space_id = backend
        .repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members = vec![
        owner,
        SpaceMemberV1::new(
            peer,
            "peer".to_owned(),
            MemberCapabilities::new(true, false),
        )?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let joined = SpaceManifestMembership::new(members, Vec::new());
    let departing = |byte: u8| -> Result<OwnedMemberRevocation, ma2a_core::ProtocolError> {
        Ok(OwnedMemberRevocation {
            space_id,
            endpoint_id: peer,
            issued_at_ms: 3_000,
            local_endpoint_id: local,
            request_id: Some(RequestId::try_from([byte; 16].as_slice())?),
        })
    };

    // And: the peer left with request A, joined again, and left again with request B.
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined.clone()))?;
    let first = backend.revoke_owned_space_member(departing(0x61)?)?;
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 4_000, joined))?;
    let second = backend.revoke_owned_space_member(departing(0x62)?)?;
    assert!(!first.replayed && !second.replayed);
    let revision = backend.repository.revision()?;

    // When / Then: while the peer is not a member, each retry gets its own answer.
    let retried = backend.revoke_owned_space_member(departing(0x61)?)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, first.chain);
    let retried = backend.revoke_owned_space_member(departing(0x62)?)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, second.chain);

    // And: a request with no receipt converges on the latest removal, and an owner-initiated
    // removal of a non-member is still refused.
    let unknown = backend.revoke_owned_space_member(departing(0x63)?)?;
    assert!(unknown.replayed);
    assert_eq!(unknown.chain, second.chain);
    let removal = OwnedMemberRevocation {
        request_id: None,
        ..departing(0x64)?
    };
    let refused = backend.revoke_owned_space_member(removal);
    assert!(refused.is_err_and(|error| error.is_space_not_found()));
    assert_eq!(backend.repository.revision()?, revision);
    Ok(())
}

#[test]
fn a_late_departure_answered_with_the_owners_removal_never_removes_a_later_membership() -> TestResult
{
    // Given: a Space this Runtime owns; the peer joined, and the owner removed it.
    let state = State::new()?;
    let mut backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
    let local = backend.initialize()?.endpoint_id;
    let owner = SpaceMemberV1::new(
        local,
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let peer_key = SpaceAuthoritySecret::from_bytes([0x44; 32]).public_key();
    let peer = EndpointId::try_from(peer_key.as_bytes().as_slice())?;
    let space_id = backend
        .repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members = vec![
        owner,
        SpaceMemberV1::new(
            peer,
            "peer".to_owned(),
            MemberCapabilities::new(true, false),
        )?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let joined = SpaceManifestMembership::new(members, Vec::new());
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 2_000, joined.clone()))?;
    let departure = OwnedMemberRevocation {
        space_id,
        endpoint_id: peer,
        issued_at_ms: 3_000,
        local_endpoint_id: local,
        request_id: Some(RequestId::try_from([0x71; 16].as_slice())?),
    };
    let removed = backend.revoke_owned_space_member(OwnedMemberRevocation {
        request_id: None,
        ..departure
    })?;

    // When: the peer's departure, sent before it learned of the removal, arrives late, and
    // the peer then joins again through a new invitation.
    let late = backend.revoke_owned_space_member(departure)?;
    assert!(late.replayed);
    assert_eq!(late.chain, removed.chain);
    backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(space_id, 4_000, joined))?;
    let rejoined = backend
        .repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    let revision = backend.repository.revision()?;

    // Then: a retry of that departure gets the same answer, and the new membership stays.
    let retried = backend.revoke_owned_space_member(departure)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, removed.chain);
    let after = backend
        .repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    assert_eq!(after, rejoined);
    assert_eq!(backend.repository.revision()?, revision);
    Ok(())
}
