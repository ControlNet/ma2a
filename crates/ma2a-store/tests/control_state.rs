//! Control synchronization repository snapshot tests.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{AddressAdvance, OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use support::{TempState, TestResult};

#[test]
fn control_snapshot_contains_only_shared_space_high_water_state() -> TestResult {
    // Given
    let state = TempState::new("control-state")?;
    let config = StoreConfig::new(state.path());
    let local = SecretKey::from_bytes(&[0x11; 32]).public().into();
    let peer = SecretKey::from_bytes(&[0x22; 32]).public().into();
    let outsider = SecretKey::from_bytes(&[0x33; 32]).public().into();
    let mut repository = Repository::open(&config)?;
    let local_member = SpaceMemberV1::new(
        local,
        "local".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let peer_member = SpaceMemberV1::new(
        peer,
        "peer".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let outsider_member = SpaceMemberV1::new(
        outsider,
        "outsider".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        local_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local_member.clone(), peer_member];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        11,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    repository.advance_address(&AddressAdvance {
        space_id: created.space_id(),
        endpoint_id: peer,
        sequence: 7,
        issued_at_ms: 12,
        expires_at_ms: 100,
        record_hash: [0x44; 32],
        signed_record: vec![0x55; 16],
    })?;
    let isolated = repository.create_owned_space(&SpaceCreation::new(
        12,
        local_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut isolated_members = vec![local_member.clone(), outsider_member];
    isolated_members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        isolated.space_id(),
        13,
        SpaceManifestMembership::new(isolated_members, Vec::new()),
    ))?;

    // When
    let shared = repository.control_spaces_between(local, peer)?;
    let outsider_shared = repository.control_spaces_between(local, outsider)?;

    // Then
    assert_eq!(shared.len(), 1);
    let shared_space = shared.first().ok_or("missing shared Space")?;
    assert_eq!(shared_space.chain().latest_generation(), 1);
    let cursor = shared_space.cursor()?;
    let address_cursor = cursor
        .address_cursors()
        .first()
        .ok_or("missing address cursor")?;
    assert_eq!(address_cursor.sequence(), 7);
    assert_eq!(outsider_shared.len(), 1);
    assert_eq!(
        outsider_shared
            .first()
            .ok_or("missing isolated shared Space")?
            .chain()
            .space_id(),
        isolated.space_id()
    );
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        14,
        SpaceManifestMembership::new(vec![local_member], vec![SpaceRevocationV1::new(peer)]),
    ))?;
    assert!(repository.control_spaces_between(local, peer)?.is_empty());
    Ok(())
}
