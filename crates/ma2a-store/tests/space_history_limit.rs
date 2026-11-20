//! A locally owned Space stops signing before its history can no longer be handed to anyone,
//! and keeps its last generations for revocations.
#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    EndpointId, EnrollmentPage, MemberCapabilities, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig, StoreError};
use support::{TempState, TestResult};

/// Two valid Ed25519 public keys: the owner's and the peer's.
const OWNER: [u8; 32] = [
    0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba, 0xa0, 0x1f,
    0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3, 0x7f, 0xcc, 0x1f, 0x9d,
];
const PEER: [u8; 32] = [
    0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab, 0x6c,
    0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81, 0xdb, 0x12,
];

fn member(endpoint: [u8; 32], name: &str) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    SpaceMemberV1::new(
        EndpointId::try_from(endpoint.as_slice())?,
        name.to_owned(),
        MemberCapabilities::new(true, endpoint == OWNER),
    )
}

#[test]
fn ordinary_changes_stop_short_of_the_limit_and_revocations_still_fit() -> TestResult {
    // Given: an owned Space whose history holds 239 ordinary manifests.
    let state = TempState::new("space-history-limit")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let (owner, peer) = (member(OWNER, "owner")?, member(PEER, "peer")?);
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let space_id = created.space_id();
    // An ordinary change renames the peer; removing a member always revokes it.
    let toggle = |generation: u64| -> Result<OwnedSpaceUpdate, ma2a_core::ProtocolError> {
        let name = if generation % 2 == 1 {
            "peer"
        } else {
            "peer-renamed"
        };
        let mut members = vec![owner.clone(), member(PEER, name)?];
        members.sort_by_key(SpaceMemberV1::endpoint_id);
        Ok(OwnedSpaceUpdate::new(
            space_id,
            2_000 + generation,
            SpaceManifestMembership::new(members, vec![]),
        ))
    };
    for generation in 1..=239 {
        repository.advance_owned_space(&toggle(generation)?)?;
    }
    let revision = repository.revision()?;

    // When / Then: generation 240 is refused for an ordinary change, without a commit.
    assert!(matches!(
        repository.advance_owned_space(&toggle(240)?),
        Err(StoreError::SpaceHistoryFull)
    ));
    assert_eq!(repository.revision()?, revision);

    // When / Then: removing and revoking the peer still fits, and the result can be exported
    // and paged.
    let revoked = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        3_000,
        SpaceManifestMembership::new(
            vec![owner.clone()],
            vec![SpaceRevocationV1::new(peer.endpoint_id())],
        ),
    ))?;
    assert_eq!(revoked.chain().latest_generation(), 240);
    revoked.chain().export_public()?;
    EnrollmentPage::paginate(revoked.chain())?;
    Ok(())
}
