//! A locally owned Space stops signing before its history can no longer be handed to anyone,
//! keeps its last generations for revocations, and never admits a change that would leave a
//! member it could not remove on its own.
#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    EndpointId, EnrollmentPage, MemberCapabilities, SpaceAuthoritySecret, SpaceId,
    SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
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

/// The `index`th of many distinct members, each with a valid Ed25519 public key.
///
/// Index 65 is the owner: [`OWNER`] is the public key of the seed `[65; 32]`.
fn numbered(index: u8) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let key = SpaceAuthoritySecret::from_bytes([index; 32]).public_key();
    SpaceMemberV1::new(
        EndpointId::try_from(key.as_bytes().as_slice())?,
        format!("member-{index}"),
        MemberCapabilities::new(true, false),
    )
}

/// One manifest of `members` beside the owner, with `revoked` revoked.
fn update(
    space_id: SpaceId,
    (owner, members): (&SpaceMemberV1, &[SpaceMemberV1]),
    (issued_at_ms, revoked): (u64, &[SpaceRevocationV1]),
) -> OwnedSpaceUpdate {
    let mut all = members.to_vec();
    all.push(owner.clone());
    all.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut revocations = revoked.to_vec();
    revocations.sort_by_key(|revocation| revocation.endpoint_id());
    OwnedSpaceUpdate::new(
        space_id,
        issued_at_ms,
        SpaceManifestMembership::new(all, revocations),
    )
}

#[test]
fn many_members_near_the_limit_can_each_still_be_removed() -> TestResult {
    // Given: 17 members beside the owner, and ordinary changes up to generation 238.
    let state = TempState::new("space-history-members")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let owner = member(OWNER, "owner")?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let space_id = created.space_id();
    let mut members = (1..=17).map(numbered).collect::<Result<Vec<_>, _>>()?;
    for generation in 1..=238_u64 {
        let mut renamed = members.clone();
        if let Some(first) = renamed.first_mut() {
            *first = SpaceMemberV1::new(
                first.endpoint_id(),
                format!("renamed-{}", generation % 2),
                MemberCapabilities::new(true, false),
            )?;
        }
        repository.advance_owned_space(&update(space_id, (&owner, &renamed), (generation, &[])))?;
    }

    // When / Then: one more ordinary change would leave 17 members and 16 generations.
    let revision = repository.revision()?;
    assert!(matches!(
        repository.advance_owned_space(&update(space_id, (&owner, &members), (500, &[]))),
        Err(StoreError::SpaceHistoryFull)
    ));
    assert_eq!(repository.revision()?, revision);

    // When / Then: every member is still removed one by one until only the owner is left.
    let mut revoked = Vec::new();
    while let Some(leaving) = members.pop() {
        revoked.push(SpaceRevocationV1::new(leaving.endpoint_id()));
        repository.advance_owned_space(&update(space_id, (&owner, &members), (600, &revoked)))?;
    }
    let chain = repository
        .load_space_chain(space_id)?
        .ok_or("the Space is gone")?;
    assert_eq!((chain.latest_generation(), chain.members().len()), (255, 1));
    Ok(())
}

#[test]
fn a_member_is_refused_once_no_revocation_entry_is_left_to_remove_it() -> TestResult {
    // Given: 64 different members admitted and removed, one after another.
    let state = TempState::new("space-history-revocations")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let owner = member(OWNER, "owner")?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let space_id = created.space_id();
    let mut revoked = Vec::new();
    for index in 1..=64 {
        let joining = numbered(index)?;
        repository.advance_owned_space(&update(
            space_id,
            (&owner, std::slice::from_ref(&joining)),
            (2_000, &revoked),
        ))?;
        revoked.push(SpaceRevocationV1::new(joining.endpoint_id()));
        repository.advance_owned_space(&update(space_id, (&owner, &[]), (2_001, &revoked)))?;
    }

    // When / Then: one more member could never be revoked, so it is not admitted.
    let revision = repository.revision()?;
    let admitted = repository.advance_owned_space(&update(
        space_id,
        (&owner, &[numbered(66)?]),
        (3_000, &revoked),
    ));
    assert!(
        matches!(admitted, Err(StoreError::SpaceHistoryFull)),
        "{:?}",
        admitted.map(|advanced| advanced.chain().latest_generation())
    );
    assert_eq!(repository.revision()?, revision);
    Ok(())
}

#[test]
fn only_a_change_that_keeps_some_of_the_members_counts_as_a_removal() -> TestResult {
    // Given: two members beside the owner, and ordinary changes up to generation 239.
    let state = TempState::new("space-history-swap")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let owner = member(OWNER, "owner")?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let space_id = created.space_id();
    let members = [numbered(1)?, numbered(2)?];
    for generation in 1..=239_u64 {
        let mut renamed = members.clone();
        renamed[0] = SpaceMemberV1::new(
            renamed[0].endpoint_id(),
            format!("renamed-{}", generation % 2),
            MemberCapabilities::new(true, false),
        )?;
        repository.advance_owned_space(&update(space_id, (&owner, &renamed), (generation, &[])))?;
    }
    let revision = repository.revision()?;

    // When / Then: revoking one member while admitting another, at the same count, is refused.
    let revoked = [SpaceRevocationV1::new(members[0].endpoint_id())];
    let swapped = [members[1].clone(), numbered(3)?];
    assert!(matches!(
        repository.advance_owned_space(&update(space_id, (&owner, &swapped), (500, &revoked))),
        Err(StoreError::SpaceHistoryFull)
    ));
    assert_eq!(repository.revision()?, revision);

    // When / Then: removing both members in one generation is a removal, so it fits.
    let both = [
        SpaceRevocationV1::new(members[0].endpoint_id()),
        SpaceRevocationV1::new(members[1].endpoint_id()),
    ];
    let removed = repository.advance_owned_space(&update(space_id, (&owner, &[]), (600, &both)))?;
    assert_eq!(removed.chain().latest_generation(), 240);
    Ok(())
}
