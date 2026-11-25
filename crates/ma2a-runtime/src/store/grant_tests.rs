//! Only the Space owner answers a grant, even one that changes nothing.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1};
use ma2a_store::{OwnedSpaceUpdate, SpaceCreation, StoreConfig};

use super::{GrantRefusal, OwnedMemberGrant};
use crate::store::StoreBackend;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

/// A private state directory removed on drop.
struct State(PathBuf);

impl State {
    fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-grant-owner-{}-{serial}", std::process::id()));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test state directory")?;
        Ok(Self(path))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_member_that_is_not_the_owner_is_refused_an_unchanged_grant() -> TestResult {
    // Given: a Space the owner signed, which the member holds without its authority.
    let (owner_state, member_state) = (State::new()?, State::new()?);
    let mut owner = StoreBackend::open(&StoreConfig::new(&owner_state.0))?;
    let mut member = StoreBackend::open(&StoreConfig::new(&member_state.0))?;
    let (owner_id, member_id) = (
        owner.initialize()?.endpoint_id,
        member.initialize()?.endpoint_id,
    );
    let signed = |id, label: &str| {
        SpaceMemberV1::new(id, label.to_owned(), MemberCapabilities::new(true, false))
    };
    let space_id = owner
        .repository
        .create_owned_space(&SpaceCreation::new(
            1_000,
            signed(owner_id, "owner")?,
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members = vec![signed(owner_id, "owner")?, signed(member_id, "member")?];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    owner
        .repository
        .advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            2_000,
            SpaceManifestMembership::new(members, Vec::new()),
        ))?;
    let chain = owner
        .repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    member.repository.persist_space_chain(&chain)?;
    let grant = |endpoint_id, local_endpoint_id| OwnedMemberGrant {
        space_id,
        endpoint_id,
        services: [false; 8],
        issued_at_ms: 3_000,
        local_endpoint_id,
    };

    // When / Then: the member is refused for either target, and nothing is recorded.
    let revision = member.repository.revision()?;
    for target in [owner_id, member_id] {
        let refused = member.grant_owned_space_member(grant(target, member_id))?;
        assert_eq!(refused.err(), Some(GrantRefusal::NotOwner));
    }
    assert_eq!(member.repository.revision()?, revision);

    // And: the owner's unchanged grant still succeeds without signing anything.
    let unchanged = owner
        .grant_owned_space_member(grant(member_id, owner_id))?
        .map_err(|refusal| format!("{refusal:?}"))?;
    assert!(!unchanged.advanced);
    Ok(())
}
