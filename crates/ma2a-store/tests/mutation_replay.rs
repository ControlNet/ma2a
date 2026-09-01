//! Durable bounded local mutation replay retention tests.

#[path = "common/support.rs"]
mod support;

use ma2a_core::RequestId;
use ma2a_store::{
    LOCAL_MUTATION_REPLAY_MAX_BYTES, LOCAL_MUTATION_REPLAY_MAX_ENTRIES,
    LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES, MutationReplayRecord, MutationReplayRequest,
    Repository, StoreConfig,
};
use rusqlite::Connection;
use support::{TempState, TestResult};

fn request_id(value: u16) -> Result<RequestId, ma2a_core::ProtocolError> {
    RequestId::try_from(value.to_be_bytes().repeat(8).as_slice())
}

fn record(
    value: u16,
    result: Vec<u8>,
) -> Result<MutationReplayRecord, Box<dyn std::error::Error + Send + Sync>> {
    Ok(MutationReplayRecord::new(
        MutationReplayRequest::new(request_id(value)?, [u8::try_from(value % 251)?; 32]),
        7,
        result,
    )?)
}

#[test]
fn replay_entries_survive_reopen_and_evict_oldest_by_count() -> TestResult {
    // Given
    let state = TempState::new("mutation-replay-count")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    for value in 0..=u16::try_from(LOCAL_MUTATION_REPLAY_MAX_ENTRIES)? {
        let replay = record(value, vec![b'x'])?;
        repository.reserve_mutation_replay(replay.request_id(), replay.fingerprint())?;
        repository.record_mutation_replay(&replay)?;
    }
    drop(repository);

    // When
    let repository = Repository::open(&config)?;

    // Then
    assert!(repository.mutation_replay(request_id(0)?)?.is_none());
    let retained = repository
        .mutation_replay(request_id(1)?)?
        .ok_or("oldest retained replay missing")?;
    let ma2a_store::MutationReplayState::Completed(retained) = retained else {
        return Err("retained replay did not complete".into());
    };
    assert_eq!(retained.response(), b"x");
    assert_eq!(replay_count(&config)?, LOCAL_MUTATION_REPLAY_MAX_ENTRIES);
    Ok(())
}

#[test]
fn replay_entries_evict_oldest_until_byte_budget_is_met() -> TestResult {
    // Given
    let state = TempState::new("mutation-replay-bytes")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let entry_count = LOCAL_MUTATION_REPLAY_MAX_BYTES
        .checked_div(LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES)
        .ok_or("invalid replay byte budget")?;
    for value in 0..=u16::try_from(entry_count)? {
        let replay = record(value, vec![b'y'; LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES])?;
        repository.reserve_mutation_replay(replay.request_id(), replay.fingerprint())?;
        repository.record_mutation_replay(&replay)?;
    }
    drop(repository);

    // When
    let repository = Repository::open(&config)?;

    // Then
    assert!(repository.mutation_replay(request_id(0)?)?.is_none());
    assert!(repository.mutation_replay(request_id(1)?)?.is_some());
    assert!(replay_bytes(&config)? <= LOCAL_MUTATION_REPLAY_MAX_BYTES);
    Ok(())
}

#[test]
fn replay_insert_and_required_eviction_are_one_transaction() -> TestResult {
    // Given
    let state = TempState::new("mutation-replay-atomicity")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let full_entries = LOCAL_MUTATION_REPLAY_MAX_BYTES
        .checked_div(LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES)
        .ok_or("invalid replay byte budget")?;
    for value in 0..u16::try_from(full_entries)? {
        let replay = record(value, vec![b'z'; LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES])?;
        repository.reserve_mutation_replay(replay.request_id(), replay.fingerprint())?;
        repository.record_mutation_replay(&replay)?;
    }
    let rejected_id = u16::try_from(full_entries)?;
    let rejected = record(rejected_id, vec![b'z'])?;
    repository.reserve_mutation_replay(rejected.request_id(), rejected.fingerprint())?;
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_replay_eviction BEFORE DELETE ON local_mutation_replay
         BEGIN SELECT RAISE(ABORT, 'reject replay eviction'); END;",
    )?;
    drop(connection);
    let mut repository = Repository::open(&config)?;

    // When
    let result = repository.record_mutation_replay(&rejected);
    drop(repository);

    // Then
    assert!(result.is_err());
    let repository = Repository::open(&config)?;
    assert!(repository.mutation_replay(request_id(0)?)?.is_some());
    assert_eq!(
        repository.mutation_replay(request_id(rejected_id)?)?,
        Some(ma2a_store::MutationReplayState::Pending(
            rejected.fingerprint()
        ))
    );
    assert_eq!(replay_count(&config)?, full_entries + 1);
    assert_eq!(replay_bytes(&config)?, LOCAL_MUTATION_REPLAY_MAX_BYTES);
    Ok(())
}

#[test]
fn pending_replay_survives_reopen_without_a_success_result() -> TestResult {
    // Given
    let state = TempState::new("mutation-replay-pending")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let id = request_id(9)?;
    repository.reserve_mutation_replay(id, [9; 32])?;
    drop(repository);

    // When
    let repository = Repository::open(&config)?;

    // Then
    assert_eq!(
        repository.mutation_replay(id)?,
        Some(ma2a_store::MutationReplayState::Pending([9; 32]))
    );
    Ok(())
}

fn replay_count(config: &StoreConfig) -> Result<usize, rusqlite::Error> {
    Connection::open(config.database_path())?.query_row(
        "SELECT COUNT(*) FROM local_mutation_replay",
        [],
        |row| row.get(0),
    )
}

fn replay_bytes(config: &StoreConfig) -> Result<usize, rusqlite::Error> {
    Connection::open(config.database_path())?.query_row(
        "SELECT COALESCE(SUM(length(response)), 0) FROM local_mutation_replay",
        [],
        |row| row.get(0),
    )
}
