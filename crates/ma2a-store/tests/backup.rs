//! Online `SQLite` backup integrity coverage.

#[path = "common/support.rs"]
mod support;

use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use rusqlite::Connection;
use support::{TempState, TestResult};

#[test]
fn online_backup_reopens_with_integrity_and_committed_state() -> TestResult {
    // Given
    let state = TempState::new("backup")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&SpaceRecord::new(space_id(), b"genesis".to_vec(), None))?;
    let destination = state.path().join("backup.sqlite3");

    // When
    repository.backup_to(&destination)?;

    // Then
    let backup = Connection::open(destination)?;
    assert_eq!(
        backup.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    assert_eq!(
        backup.query_row("SELECT COUNT(*) FROM spaces", [], |row| row
            .get::<_, u32>(0))?,
        1
    );
    Ok(())
}

fn space_id() -> ma2a_core::SpaceId {
    ma2a_core::SpaceId::derive(b"ma2a-store-test-genesis")
}
