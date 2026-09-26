//! The local authority must preserve the genesis owner before signing.
#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use support::{TempState, TestResult};

#[test]
fn owned_update_cannot_remove_genesis_owner() -> TestResult {
    let state = TempState::new("owner-invariant")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let owner = member()?;
    let peer = peer_member()?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![owner.clone(), peer.clone()];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let before = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        2000,
        SpaceManifestMembership::new(members, vec![]),
    ))?;
    let authority = authority_snapshot(&config)?;
    for generation in 1..=3 {
        let before = repository
            .load_space_chain(created.space_id())?
            .ok_or("missing chain")?;
        let revision = repository.revision()?;
        // Check both halves independently, including an otherwise malformed
        // proposal that lists the owner as both present and revoked.
        for proposal in [
            SpaceManifestMembership::new(
                vec![peer.clone()],
                vec![SpaceRevocationV1::new(owner.endpoint_id())],
            ),
            SpaceManifestMembership::new(vec![peer.clone()], vec![]),
            SpaceManifestMembership::new(
                before.members().to_vec(),
                vec![SpaceRevocationV1::new(owner.endpoint_id())],
            ),
        ] {
            let result = repository.advance_owned_space(&OwnedSpaceUpdate::new(
                created.space_id(),
                3000 + generation,
                proposal,
            ));
            assert!(matches!(
                result,
                Err(ma2a_store::StoreError::SpaceOwnerCannotBeRemoved)
            ));
            let reopened = Repository::open(&config)?;
            assert_eq!(reopened.revision()?, revision);
            assert_eq!(
                reopened.load_space_chain(created.space_id())?,
                Some(before.clone())
            );
            assert!(
                reopened
                    .memberships_for(owner.endpoint_id())?
                    .contains(&created.space_id())
            );
            let connection = rusqlite::Connection::open(config.database_path())?;
            let revoked: u64 = connection.query_row(
                "SELECT COUNT(*) FROM member_revocations WHERE endpoint_id = ?1",
                [owner.endpoint_id().as_bytes().as_slice()],
                |row| row.get(0),
            )?;
            assert_eq!(revoked, 0);
            // Avoid printing any protected material in assertion diagnostics.
            assert!(authority_snapshot(&config)? == authority);
        }
        if generation < 3 {
            let membership = if generation == 1 {
                SpaceManifestMembership::new(
                    vec![owner.clone()],
                    vec![SpaceRevocationV1::new(peer.endpoint_id())],
                )
            } else {
                SpaceManifestMembership::new(vec![peer.clone(), owner.clone()], vec![])
            };
            let advanced = repository.advance_owned_space(&OwnedSpaceUpdate::new(
                created.space_id(),
                4000 + generation,
                membership,
            ))?;
            assert_eq!(advanced.revision(), revision + 1);
            assert_eq!(advanced.chain().latest_generation(), generation + 1);
        }
    }
    assert_eq!(before.chain().latest_generation(), 1);
    Ok(())
}

fn member() -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let endpoint = ma2a_core::EndpointId::try_from(
        [
            0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba,
            0xa0, 0x1f, 0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3,
            0x7f, 0xcc, 0x1f, 0x9d,
        ]
        .as_slice(),
    )?;
    SpaceMemberV1::new(
        endpoint,
        "repository-owner".to_owned(),
        MemberCapabilities::new(true, true),
    )
}

fn peer_member() -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let endpoint = ma2a_core::EndpointId::try_from(
        [
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
        ]
        .as_slice(),
    )?;
    SpaceMemberV1::new(
        endpoint,
        "repository-peer".to_owned(),
        MemberCapabilities::new(true, false),
    )
}

fn authority_snapshot(
    config: &StoreConfig,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let connection = rusqlite::Connection::open(config.database_path())?;
    let reference: String =
        connection.query_row("SELECT authority_key_ref FROM spaces", [], |row| row.get(0))?;
    let bytes = std::fs::read(
        config
            .database_path()
            .parent()
            .ok_or("missing state directory")?
            .join("keys/space-authority")
            .join(format!("{reference}.key")),
    )?;
    Ok((reference, bytes))
}

#[test]
fn generic_signed_owner_removal_remains_importable() -> TestResult {
    let state = TempState::new("imported-owner-removal")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let owner = member()?;
    let peer = peer_member()?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let added = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        2000,
        SpaceManifestMembership::new(vec![peer.clone(), owner.clone()], vec![]),
    ))?;
    // Adversarial signed-chain construction deliberately bypasses local signing.
    // This also documents that existing invalid Phase-1 owned chains still load.
    let (_, bytes) = authority_snapshot(&config)?;
    let secret = ma2a_core::SpaceAuthoritySecret::try_from_bytes(&bytes)?;
    let mut chain = added.chain().clone();
    let manifest = ma2a_core::SpaceManifestV1::new(
        ma2a_core::SpaceManifestLink::new(created.space_id(), 2, chain.latest_hash()),
        3000,
        SpaceManifestMembership::new(
            vec![peer],
            vec![SpaceRevocationV1::new(owner.endpoint_id())],
        ),
    )?
    .sign(&secret)?;
    chain.apply(&manifest)?;
    assert!(repository.persist_space_chain(&chain)?.error().is_none());
    drop(repository);
    let reopened = Repository::open(&config)?;
    assert_eq!(reopened.load_space_chain(created.space_id())?, Some(chain));
    assert!(
        !reopened
            .memberships_for(owner.endpoint_id())?
            .contains(&created.space_id())
    );

    Ok(())
}
