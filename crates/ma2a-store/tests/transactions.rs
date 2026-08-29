//! Real `SQLite` transaction, rollback, reopen, and contention coverage.

#[path = "common/space_fixture.rs"]
mod space_fixture;
#[path = "transactions/state.rs"]
mod state;
#[path = "common/support.rs"]
mod support;

use std::{
    io::{BufRead as _, BufReader, Write as _},
    process::{Command, Stdio},
    sync::{Arc, Barrier},
};

use ma2a_store::{
    InvitationRecord, ManifestOutcome, Redemption, RedemptionOutcome, Repository, StoreConfig,
};
use rusqlite::Connection;
use support::{TempState, TestResult, TestResultValue};

const CRASH_CHILD_ENV: &str = "MA2A_STORE_CRASH_CHILD";
const CRASH_STATE_ENV: &str = "MA2A_STORE_CRASH_STATE";
const CRASH_READY: &str = "MA2A_STORE_CRASH_READY";

#[test]
fn manifest_conflict_rolls_back_manifest_and_revision() -> TestResult {
    // Given
    let state = TempState::new("manifest-rollback")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&space_fixture::space_record()?)?;
    let revision = repository.revision()?;
    let advance = manifest_advance(2, [1; 32])?;

    // When
    let outcome = repository.advance_manifest(&advance)?;

    // Then
    assert_eq!(
        outcome,
        ManifestOutcome::Conflict {
            current_generation: Some(0)
        }
    );
    assert_eq!(repository.revision()?, revision);
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM manifests", [], |row| row
            .get::<_, u32>(0))?,
        1
    );
    Ok(())
}

#[test]
fn identical_manifest_replay_is_idempotent_without_revision_change() -> TestResult {
    let state = TempState::new("manifest-idempotent-replay")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let genesis = space_fixture::signed_space_genesis()?;
    repository.create_space(&space_fixture::space_record()?)?;
    let advance = manifest_advance(1, genesis.chain_hash())?;
    let first = repository.advance_manifest(&advance)?;
    assert!(matches!(first, ManifestOutcome::Advanced { .. }));
    let revision = repository.revision()?;

    assert_eq!(
        repository.advance_manifest(&advance)?,
        ManifestOutcome::Idempotent {
            current_generation: 1
        }
    );
    assert_eq!(repository.revision()?, revision);
    Ok(())
}

#[test]
fn forced_termination_rolls_back_uncommitted_transaction() -> TestResult {
    if std::env::var_os(CRASH_CHILD_ENV).is_some() {
        return run_crash_child();
    }

    // Given
    let state = TempState::new("crash-reopen")?;
    let config = StoreConfig::new(state.path());
    drop(Repository::open(&config)?);
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "forced_termination_rolls_back_uncommitted_transaction",
            "--nocapture",
        ])
        .env(CRASH_CHILD_ENV, "1")
        .env(CRASH_STATE_ENV, state.path())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or("crash child stdout was not captured")?;
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = lines
            .next()
            .ok_or("crash child exited before readiness")??;
        if line == CRASH_READY {
            break;
        }
    }

    // When
    child.kill()?;
    let status = child.wait()?;
    assert!(!status.success());
    let repository = Repository::open(&config)?;

    // Then
    assert_eq!(repository.revision()?, 0);
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM spaces", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    Ok(())
}

fn run_crash_child() -> TestResult {
    let state_path =
        std::env::var_os(CRASH_STATE_ENV).ok_or("crash child state path is missing")?;
    let config = StoreConfig::new(std::path::PathBuf::from(state_path));
    drop(Repository::open(&config)?);
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    connection.execute(
        "INSERT INTO spaces(space_id, genesis_cbor) VALUES (?1, ?2)",
        (space_id()?.as_bytes().as_slice(), b"uncommitted".as_slice()),
    )?;
    connection.execute(
        "UPDATE runtime_metadata SET revision = revision + 1 WHERE singleton = 1",
        [],
    )?;
    println!("{CRASH_READY}");
    std::io::stdout().flush()?;
    loop {
        std::thread::park();
    }
}

#[test]
fn two_concurrent_redemptions_have_exactly_one_winner() -> TestResult {
    // Given
    let state = TempState::new("concurrent-redemption")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&space_fixture::space_record()?)?;
    repository.create_invitation(&InvitationRecord::new(
        [3; 16],
        space_id()?,
        [4; 32],
        endpoint_id()?,
        1,
        10_000,
        vec![1],
    ))?;
    drop(repository);
    let barrier = Arc::new(Barrier::new(3));

    // When
    let first = redeem_on_thread(config.clone(), Arc::clone(&barrier), 1)?;
    let second = redeem_on_thread(config, Arc::clone(&barrier), 2)?;
    barrier.wait();
    let outcomes = [join(first)?, join(second)?];

    // Then
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| matches!(value, RedemptionOutcome::Redeemed { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| **value == RedemptionOutcome::AlreadyConsumed)
            .count(),
        1
    );
    Ok(())
}

#[test]
fn expired_invitation_is_persistently_invalidated() -> TestResult {
    // Given
    let state = TempState::new("expired-invitation")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&space_fixture::space_record()?)?;
    repository.create_invitation(&InvitationRecord::new(
        [5; 16],
        space_id()?,
        [6; 32],
        endpoint_id()?,
        1,
        10,
        vec![1],
    ))?;
    let redemption = Redemption::new([6; 32], endpoint_id()?, 10, 10);

    // When
    let first = repository.redeem_invitation(&redemption)?;
    let second = repository.redeem_invitation(&redemption)?;

    // Then
    assert_eq!(first, RedemptionOutcome::Expired);
    assert_eq!(second, RedemptionOutcome::Expired);
    Ok(())
}

fn redeem_on_thread(
    config: StoreConfig,
    barrier: Arc<Barrier>,
    marker: u8,
) -> TestResultValue<std::thread::JoinHandle<Result<RedemptionOutcome, ma2a_store::StoreError>>> {
    let endpoint = endpoint_id()?;
    Ok(std::thread::spawn(move || {
        let mut repository = Repository::open(&config)?;
        barrier.wait();
        repository.redeem_invitation(&Redemption::new([4; 32], endpoint, 100, marker.into()))
    }))
}

fn join(
    handle: std::thread::JoinHandle<Result<RedemptionOutcome, ma2a_store::StoreError>>,
) -> TestResultValue<RedemptionOutcome> {
    let result = handle
        .join()
        .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
            "redemption thread panicked".into()
        })?;
    result.map_err(Into::into)
}

fn endpoint_id() -> TestResultValue<ma2a_core::EndpointId> {
    const BYTES: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66,
    ];
    Ok(ma2a_core::EndpointId::try_from(BYTES.as_slice())?)
}

fn space_id() -> TestResultValue<ma2a_core::SpaceId> {
    Ok(space_fixture::signed_space_genesis()?.space_id())
}

fn manifest_advance(
    generation: u64,
    previous_hash: [u8; 32],
) -> TestResultValue<ma2a_store::ManifestAdvance> {
    let genesis = space_fixture::signed_space_genesis()?;
    let secret = ma2a_core::SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let manifest = ma2a_core::SpaceManifestV1::new(
        ma2a_core::SpaceManifestLink::new(genesis.space_id(), generation, previous_hash),
        generation + 1,
        ma2a_core::SpaceManifestMembership::new(
            vec![genesis.genesis().initial_member().clone()],
            vec![],
        ),
    )?
    .sign(&secret)?;
    Ok(ma2a_store::ManifestAdvance::new(
        genesis.space_id(),
        manifest.generation(),
        Some(manifest.previous_hash()),
        manifest.manifest_hash(),
        manifest.canonical_bytes().to_vec(),
    ))
}
