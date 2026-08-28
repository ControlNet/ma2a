//! Real `SQLite` transaction, rollback, reopen, and contention coverage.

#[path = "transactions/state.rs"]
mod state;
#[path = "common/support.rs"]
mod support;

use std::sync::{Arc, Barrier};

use ma2a_store::{
    InvitationRecord, ManifestAdvance, ManifestOutcome, Redemption, RedemptionOutcome, Repository,
    SpaceRecord, StoreConfig,
};
use rusqlite::Connection;
use support::{TempState, TestResult, TestResultValue};

#[test]
fn manifest_conflict_rolls_back_manifest_and_revision() -> TestResult {
    // Given
    let state = TempState::new("manifest-rollback")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&SpaceRecord::new(space_id(), b"genesis".to_vec(), None))?;
    let revision = repository.revision()?;
    let advance = ManifestAdvance::new(
        space_id(),
        2,
        Some([1; 32]),
        [2; 32],
        b"signed-manifest".to_vec(),
    );

    // When
    let outcome = repository.advance_manifest(&advance)?;

    // Then
    assert_eq!(
        outcome,
        ManifestOutcome::Conflict {
            current_generation: None
        }
    );
    assert_eq!(repository.revision()?, revision);
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM manifests", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    Ok(())
}

#[test]
fn dropped_uncommitted_connection_is_empty_after_reopen() -> TestResult {
    // Given
    let state = TempState::new("crash-reopen")?;
    let config = StoreConfig::new(state.path());
    drop(Repository::open(&config)?);
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    connection.execute(
        "INSERT INTO spaces(space_id, genesis_cbor) VALUES (?1, ?2)",
        (space_id().as_bytes().as_slice(), b"uncommitted".as_slice()),
    )?;

    // When
    drop(connection);
    drop(Repository::open(&config)?);

    // Then
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM spaces", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    Ok(())
}

#[test]
fn two_concurrent_redemptions_have_exactly_one_winner() -> TestResult {
    // Given
    let state = TempState::new("concurrent-redemption")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&SpaceRecord::new(space_id(), b"genesis".to_vec(), None))?;
    repository.create_invitation(&InvitationRecord::new([3; 16], space_id(), [4; 32], 10_000))?;
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
    repository.create_space(&SpaceRecord::new(space_id(), b"genesis".to_vec(), None))?;
    repository.create_invitation(&InvitationRecord::new([5; 16], space_id(), [6; 32], 10))?;
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

fn space_id() -> ma2a_core::SpaceId {
    ma2a_core::SpaceId::derive(b"ma2a-store-test-genesis")
}
