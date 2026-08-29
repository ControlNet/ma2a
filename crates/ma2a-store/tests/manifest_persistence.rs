//! Persistence acceptance tests for complete signed Space chains.

#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    ManifestError, MemberCapabilities, SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity,
    SpaceGenesisOwner, SpaceGenesisV1, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
    SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_store::{Repository, StoreConfig};
use rusqlite::Connection;
use support::{TempState, TestResult};

const TEST_AUTHORITY: [u8; 32] = [0x61; 32];

#[test]
fn full_chain_and_latest_state_survive_reopen_without_regression() -> TestResult {
    let state = TempState::new("manifest-chain-reopen")?;
    let config = StoreConfig::new(state.path());
    let mut chain = chain_through_generation_two()?;
    let expected_hash = chain.latest_hash();

    let mut repository = Repository::open(&config)?;
    let outcome = repository.persist_space_chain(&chain)?;
    assert!(outcome.advanced());
    let revision = repository.revision()?;
    drop(repository);

    let mut repository = Repository::open(&config)?;
    let loaded = repository
        .load_space_chain(chain.space_id())?
        .ok_or("missing chain")?;
    assert_eq!(loaded, chain);
    assert_eq!(loaded.latest_generation(), 2);
    assert_eq!(loaded.latest_hash(), expected_hash);

    let first = chain
        .manifests()
        .first()
        .ok_or("missing first manifest")?
        .clone();
    chain = SpaceChain::from_genesis(chain.genesis().clone())?;
    chain.apply(&first)?;
    assert_eq!(
        repository.persist_space_chain(&chain)?.error(),
        Some(ManifestError::ROLLBACK)
    );
    assert_eq!(repository.revision()?, revision);
    Ok(())
}

#[test]
fn rejected_fork_skip_and_rollback_leave_rows_and_revision_unchanged() -> TestResult {
    let state = TempState::new("manifest-rejected-updates")?;
    let config = StoreConfig::new(state.path());
    let chain = chain_through_generation_two()?;
    let mut repository = Repository::open(&config)?;
    repository.persist_space_chain(&chain)?;
    let revision = repository.revision()?;

    let genesis_only = SpaceChain::from_genesis(chain.genesis().clone())?;
    assert_eq!(
        repository.persist_space_chain(&genesis_only)?.error(),
        Some(ManifestError::ROLLBACK)
    );

    let mut fork = SpaceChain::from_genesis(chain.genesis().clone())?;
    let secret = SpaceAuthoritySecret::from_bytes(TEST_AUTHORITY);
    let forked = SpaceManifestV1::new(
        SpaceManifestLink::new(fork.space_id(), 1, fork.latest_hash()),
        99,
        SpaceManifestMembership::new(vec![member(0x66, true)?], vec![]),
    )?
    .sign(&secret)?;
    fork.apply(&forked)?;
    assert_eq!(
        repository.persist_space_chain(&fork)?.error(),
        Some(ManifestError::FORK)
    );
    assert_eq!(repository.revision()?, revision);
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM manifests", [], |row| row
            .get::<_, u32>(0))?,
        3
    );
    assert_eq!(
        connection.query_row("SELECT latest_manifest_generation FROM spaces", [], |row| {
            row.get::<_, u64>(0)
        })?,
        2
    );
    Ok(())
}

#[test]
fn public_export_import_excludes_private_authority_material() -> TestResult {
    let state = TempState::new("manifest-export-import")?;
    let source_config = StoreConfig::new(state.path().join("source"));
    let target_config = StoreConfig::new(state.path().join("target"));
    let chain = chain_through_generation_two()?;
    let mut source = Repository::open(&source_config)?;
    source.persist_space_chain(&chain)?;

    let exported = source
        .export_space_chain(chain.space_id())?
        .ok_or("missing export")?;
    assert!(!exported.windows(32).any(|window| window == TEST_AUTHORITY));
    drop(source);

    let mut target = Repository::open(&target_config)?;
    let imported = target.import_space_chain(&exported)?;
    assert!(imported.advanced());
    drop(target);
    let reopened = Repository::open(&target_config)?;
    assert_eq!(reopened.load_space_chain(chain.space_id())?, Some(chain));
    Ok(())
}

#[test]
fn reopen_rejects_tampered_manifest_row_metadata() -> TestResult {
    let state = TempState::new("manifest-row-metadata")?;
    let config = StoreConfig::new(state.path());
    let chain = chain_through_generation_two()?;
    let mut repository = Repository::open(&config)?;
    repository.persist_space_chain(&chain)?;
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    connection.execute(
        "UPDATE manifests SET previous_hash = ?1 WHERE generation = 1",
        [[0x99; 32].as_slice()],
    )?;
    drop(connection);

    assert!(Repository::open(&config).is_err());
    Ok(())
}

#[test]
fn reopen_rejects_materialized_members_that_diverge_from_signed_chain() -> TestResult {
    let state = TempState::new("manifest-derived-members")?;
    let config = StoreConfig::new(state.path());
    let chain = chain_through_generation_two()?;
    let mut repository = Repository::open(&config)?;
    repository.persist_space_chain(&chain)?;
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    connection.execute("UPDATE members SET role = 1", [])?;
    drop(connection);

    assert!(Repository::open(&config).is_err());
    Ok(())
}

#[test]
fn replacement_failure_rolls_back_chain_rows_derived_state_and_revision() -> TestResult {
    let state = TempState::new("manifest-replacement-rollback")?;
    let config = StoreConfig::new(state.path());
    let full_chain = chain_through_generation_two()?;
    let genesis_only = SpaceChain::from_genesis(full_chain.genesis().clone())?;
    let mut repository = Repository::open(&config)?;
    repository.persist_space_chain(&genesis_only)?;
    let revision = repository.revision()?;
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_replacement_members
         BEFORE INSERT ON members
         BEGIN SELECT RAISE(ABORT, 'injected member failure'); END;",
    )?;
    drop(connection);

    assert!(repository.persist_space_chain(&full_chain).is_err());
    assert_eq!(repository.revision()?, revision);
    assert_eq!(
        repository
            .load_space_chain(full_chain.space_id())?
            .ok_or("missing preserved genesis")?
            .latest_generation(),
        0
    );
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM manifests", [], |row| row
            .get::<_, u32>(0))?,
        1
    );
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM member_revocations", [], |row| {
            row.get::<_, u32>(0)
        })?,
        0
    );
    Ok(())
}

fn chain_through_generation_two() -> Result<SpaceChain, Box<dyn std::error::Error + Send + Sync>> {
    let secret = SpaceAuthoritySecret::from_bytes(TEST_AUTHORITY);
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x62; 32], 10, secret.public_key())?,
        SpaceGenesisOwner::new(member(0x66, true)?, SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        11,
        SpaceManifestMembership::new(vec![member(0x77, false)?, member(0x66, true)?], vec![]),
    )?
    .sign(&secret)?;
    chain.apply(&first)?;
    let second = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        12,
        SpaceManifestMembership::new(
            vec![member(0x66, true)?],
            vec![SpaceRevocationV1::new(endpoint(0x77)?)],
        ),
    )?
    .sign(&secret)?;
    chain.apply(&second)?;
    Ok(chain)
}

fn member(marker: u8, relay: bool) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    SpaceMemberV1::new(
        endpoint(marker)?,
        format!("member-{marker:02x}"),
        MemberCapabilities::new(true, relay),
    )
}

fn endpoint(marker: u8) -> Result<ma2a_core::EndpointId, ma2a_core::ProtocolError> {
    let bytes = match marker {
        0x66 => [
            0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba,
            0xa0, 0x1f, 0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3,
            0x7f, 0xcc, 0x1f, 0x9d,
        ],
        0x77 => [
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
        ],
        _ => return Err(ma2a_core::ProtocolError::INVALID_INPUT),
    };
    ma2a_core::EndpointId::try_from(bytes.as_slice())
}
