//! Control synchronization repository snapshot tests.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, MemberCapabilities,
    SpaceAddressRecordV1, SpaceAuthorizationView, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, SpaceCreation,
    StoreConfig, ValidatedAddressRecord,
};
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
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("shared Space chain missing")?;
    let authorization = SpaceAuthorizationView::from_chain(&chain);
    let peer_secret = SecretKey::from_bytes(&[0x22; 32]);
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(created.space_id(), peer),
        AddressRecordValidity::new(7, 12, 100)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(&peer_secret)?;
    let validated = ValidatedAddressRecord::parse(
        signed.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(created.space_id(), peer),
            &authorization,
            50,
        ),
    )?;
    repository.advance_validated_address(&validated)?;
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
