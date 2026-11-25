use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{
    EndpointId, MemberCapabilities, RequestId, SpaceAuthoritySecret, SpaceId,
    SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{OwnedSpaceUpdate, SpaceCreation, StoreConfig};

use super::OwnedMemberRevocation;
use crate::store::StoreBackend;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
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

struct OwnedSpaceFixture {
    _state: State,
    config: StoreConfig,
    backend: StoreBackend,
    local: EndpointId,
    peer: EndpointId,
    space_id: SpaceId,
    joined: SpaceManifestMembership,
}

impl OwnedSpaceFixture {
    fn with_peer(peer_seed: u8) -> TestResult<Self> {
        let state = State::new()?;
        let config = StoreConfig::new(&state.0);
        let mut backend = StoreBackend::open(&config)?;
        let local = backend.initialize()?.endpoint_id;
        let owner = SpaceMemberV1::new(
            local,
            "owner".to_owned(),
            MemberCapabilities::new(true, false),
        )?;
        let peer_key = SpaceAuthoritySecret::from_bytes([peer_seed; 32]).public_key();
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
        Ok(Self {
            _state: state,
            config,
            backend,
            local,
            peer,
            space_id,
            joined,
        })
    }

    fn departure(&self, byte: u8) -> Result<OwnedMemberRevocation, ma2a_core::ProtocolError> {
        Ok(OwnedMemberRevocation {
            space_id: self.space_id,
            endpoint_id: self.peer,
            issued_at_ms: 3_000,
            local_endpoint_id: self.local,
            request_id: Some(RequestId::try_from([byte; 16].as_slice())?),
        })
    }
}

#[test]
fn a_retried_departure_never_removes_a_member_who_joined_again() -> TestResult {
    // Given: a Space this Runtime owns, which the peer joined, left, and joined again.
    let mut fixture = OwnedSpaceFixture::with_peer(0x42)?;
    let departure = fixture.departure(0x61)?;
    let first = fixture.backend.revoke_owned_space_member(departure)?;
    assert!(!first.replayed);
    fixture
        .backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(
            fixture.space_id,
            4_000,
            fixture.joined.clone(),
        ))?;
    let rejoined = fixture
        .backend
        .repository
        .load_space_chain(fixture.space_id)?
        .ok_or("the Space is gone")?;
    let revision = fixture.backend.repository.revision()?;

    // When: the first departure request arrives again.
    let retried = fixture.backend.revoke_owned_space_member(departure)?;

    // Then: it gets the first answer, and the second membership, chain head, and revision stay.
    assert!(retried.replayed);
    assert_eq!(retried.chain, first.chain);
    let after = fixture
        .backend
        .repository
        .load_space_chain(fixture.space_id)?
        .ok_or("the Space is gone")?;
    assert_eq!(after, rejoined);
    assert!(
        after
            .members()
            .iter()
            .any(|member| member.endpoint_id() == fixture.peer)
    );
    assert_eq!(fixture.backend.repository.revision()?, revision);
    Ok(())
}

#[test]
fn each_departure_of_a_member_who_left_twice_gets_its_own_answer() -> TestResult {
    // Given: the peer left with request A, joined again, and left again with request B.
    let mut fixture = OwnedSpaceFixture::with_peer(0x43)?;
    let first = fixture
        .backend
        .revoke_owned_space_member(fixture.departure(0x61)?)?;
    fixture
        .backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(
            fixture.space_id,
            4_000,
            fixture.joined.clone(),
        ))?;
    let second = fixture
        .backend
        .revoke_owned_space_member(fixture.departure(0x62)?)?;
    assert!(!first.replayed && !second.replayed);
    let revision = fixture.backend.repository.revision()?;

    // When / Then: while the peer is not a member, each retry gets its own answer.
    let retried = fixture
        .backend
        .revoke_owned_space_member(fixture.departure(0x61)?)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, first.chain);
    let retried = fixture
        .backend
        .revoke_owned_space_member(fixture.departure(0x62)?)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, second.chain);

    // And: a request with no receipt converges on the latest removal, and an owner-initiated
    // removal of a non-member is still refused.
    let unknown = fixture
        .backend
        .revoke_owned_space_member(fixture.departure(0x63)?)?;
    assert!(unknown.replayed);
    assert_eq!(unknown.chain, second.chain);
    let removal = OwnedMemberRevocation {
        request_id: None,
        ..fixture.departure(0x64)?
    };
    let refused = fixture.backend.revoke_owned_space_member(removal);
    assert!(refused.is_err_and(|error| error.is_space_not_found()));
    assert_eq!(fixture.backend.repository.revision()?, revision);
    Ok(())
}

#[test]
fn a_late_departure_answered_with_the_owners_removal_never_removes_a_later_membership() -> TestResult
{
    // Given: a Space this Runtime owns; the peer joined, and the owner removed it.
    let mut fixture = OwnedSpaceFixture::with_peer(0x44)?;
    let departure = fixture.departure(0x71)?;
    let removed = fixture
        .backend
        .revoke_owned_space_member(OwnedMemberRevocation {
            request_id: None,
            ..departure
        })?;

    // When: the peer's departure arrives late, and the peer then joins again.
    let late = fixture.backend.revoke_owned_space_member(departure)?;
    assert!(late.replayed);
    assert_eq!(late.chain, removed.chain);
    fixture
        .backend
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(
            fixture.space_id,
            4_000,
            fixture.joined.clone(),
        ))?;
    let rejoined = fixture
        .backend
        .repository
        .load_space_chain(fixture.space_id)?
        .ok_or("the Space is gone")?;
    let revision = fixture.backend.repository.revision()?;

    // Then: a retry of that departure gets the same answer, and the new membership stays.
    let retried = fixture.backend.revoke_owned_space_member(departure)?;
    assert!(retried.replayed);
    assert_eq!(retried.chain, removed.chain);
    let after = fixture
        .backend
        .repository
        .load_space_chain(fixture.space_id)?
        .ok_or("the Space is gone")?;
    assert_eq!(after, rejoined);
    assert_eq!(fixture.backend.repository.revision()?, revision);
    Ok(())
}

#[test]
fn a_keyless_creator_refuses_both_replayed_and_unbound_departure_receipts() -> TestResult {
    // Given: a Space where peer left (binding receipt 0x81), and the Space then lost its key.
    let mut fixture = OwnedSpaceFixture::with_peer(0x45)?;
    let bound = fixture.departure(0x81)?;
    let fresh = fixture.departure(0x82)?;
    fixture.backend.revoke_owned_space_member(bound)?;
    let config = fixture.config.clone();
    drop(fixture.backend);
    rusqlite::Connection::open(config.database_path())?
        .execute("UPDATE spaces SET authority_key_ref = NULL", [])?;
    let mut backend = StoreBackend::open(&config)?;

    // When / Then: both the previously bound receipt and a fresh request ID are refused as not
    // holding Space authority, and no new departure_receipts row is written.
    let replayed = backend.revoke_owned_space_member(bound);
    assert!(replayed.is_err_and(|error| error.is_not_space_owner()));
    let unbound = backend.revoke_owned_space_member(fresh);
    assert!(unbound.is_err_and(|error| error.is_not_space_owner()));
    let receipt_count: i64 = rusqlite::Connection::open(config.database_path())?.query_row(
        "SELECT COUNT(*) FROM departure_receipts",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(receipt_count, 1);
    Ok(())
}
