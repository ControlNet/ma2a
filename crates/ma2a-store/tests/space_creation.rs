//! Acceptance tests for repository-owned Space authority creation.

#[path = "common/support.rs"]
mod support;

use std::fs;

use ma2a_core::{
    Capability, ManifestError, MemberCapabilities, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use rusqlite::Connection;
use support::{TempState, TestResult};

#[test]
fn two_spaces_create_distinct_protected_authority_keys() -> TestResult {
    let state = TempState::new("owned-space-keys")?;
    let config = StoreConfig::new(state.path());
    let member = member()?;
    let mut repository = Repository::open(&config)?;

    let first = repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_000,
        member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let second = repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_001,
        member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;

    assert_ne!(first.space_id(), second.space_id());
    assert_ne!(
        first.chain().genesis().authority(),
        second.chain().genesis().authority()
    );
    assert!(
        first
            .authorization()
            .allows(member.endpoint_id(), Capability::ECHO)
    );
    assert!(second.revision() > first.revision());
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    let references = connection
        .prepare("SELECT authority_key_ref FROM spaces ORDER BY space_id")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(references.len(), 2);
    let first_reference = references.first().ok_or("missing first reference")?;
    let second_reference = references.get(1).ok_or("missing second reference")?;
    assert_ne!(first_reference, second_reference);

    let key_directory = state.path().join("keys/space-authority");
    let secrets = fs::read_dir(key_directory)?
        .map(|entry| fs::read(entry?.path()))
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(secrets.len(), 2);
    assert!(secrets.iter().all(|secret| secret.len() == 32));
    let first_secret = secrets.first().ok_or("missing first secret")?;
    let second_secret = secrets.get(1).ok_or("missing second secret")?;
    assert_ne!(first_secret, second_secret);
    let database = fs::read(config.database_path())?;
    assert!(secrets.iter().all(|secret| {
        !database
            .windows(secret.len())
            .any(|window| window == secret)
    }));

    let export = reopened_export(&config, first.space_id())?;
    assert!(
        secrets
            .iter()
            .all(|secret| !export.windows(secret.len()).any(|window| window == secret))
    );
    assert!(
        !export
            .windows(first_reference.len())
            .any(|window| window == first_reference.as_bytes())
    );

    let reopened = Repository::open(&config)?;
    assert_eq!(
        reopened.load_space_chain(first.space_id())?,
        Some(first.chain().clone())
    );
    assert_eq!(
        reopened.load_space_chain(second.space_id())?,
        Some(second.chain().clone())
    );
    Ok(())
}

#[test]
fn reopen_rejects_authority_key_that_no_longer_matches_genesis() -> TestResult {
    let state = TempState::new("owned-space-key-mismatch")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_000,
        member()?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    let reference = connection.query_row("SELECT authority_key_ref FROM spaces", [], |row| {
        row.get::<_, String>(0)
    })?;
    drop(connection);
    fs::write(
        state
            .path()
            .join("keys/space-authority")
            .join(format!("{reference}.key")),
        [0x55; 32],
    )?;

    assert!(Repository::open(&config).is_err());
    Ok(())
}

#[test]
fn owned_space_advance_signs_with_protected_authority_and_commits_before_authorization()
-> TestResult {
    let state = TempState::new("owned-space-advance")?;
    let config = StoreConfig::new(state.path());
    let owner = member()?;
    let peer = peer_member()?;
    let mut repository = Repository::open(&config)?;
    let first = repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let second = repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_001,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![owner.clone(), peer.clone()];
    members.sort_by_key(SpaceMemberV1::endpoint_id);

    let advanced = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        first.space_id(),
        1_700_000_001_000,
        SpaceManifestMembership::new(members, vec![]),
    ))?;
    assert_eq!(advanced.chain().latest_generation(), 1);
    assert!(
        advanced
            .authorization()
            .allows(peer.endpoint_id(), Capability::ECHO)
    );
    assert!(advanced.revision() > second.revision());
    let revoked = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        first.space_id(),
        1_700_000_002_000,
        SpaceManifestMembership::new(
            vec![owner.clone()],
            vec![SpaceRevocationV1::new(peer.endpoint_id())],
        ),
    ))?;
    assert_eq!(revoked.chain().latest_generation(), 2);
    assert!(
        !revoked
            .authorization()
            .allows(peer.endpoint_id(), Capability::ECHO)
    );
    assert!(
        second
            .authorization()
            .allows(owner.endpoint_id(), Capability::ECHO)
    );
    assert_eq!(
        repository.persist_space_chain(first.chain())?.error(),
        Some(ManifestError::ROLLBACK)
    );
    println!(
        "{{\"space_a_generation\":{},\"space_a_hash\":\"{}\",\"peer_authorized_in_a\":false,\"owner_authorized_in_b\":true}}",
        revoked.chain().latest_generation(),
        hex(revoked.chain().latest_hash())?
    );
    drop(repository);

    let reopened = Repository::open(&config)?;
    assert_eq!(
        reopened
            .load_space_chain(first.space_id())?
            .ok_or("missing advanced Space")?
            .latest_generation(),
        2
    );
    assert_eq!(
        reopened
            .load_space_chain(second.space_id())?
            .ok_or("missing unaffected Space")?
            .latest_generation(),
        0
    );
    Ok(())
}

fn hex(bytes: [u8; 32]) -> Result<String, std::fmt::Error> {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(64);
    for byte in bytes {
        write!(&mut output, "{byte:02x}")?;
    }
    Ok(output)
}

fn reopened_export(
    config: &StoreConfig,
    space_id: ma2a_core::SpaceId,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    Repository::open(config)?
        .export_space_chain(space_id)?
        .ok_or_else(|| "missing export".into())
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
