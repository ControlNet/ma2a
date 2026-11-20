//! Online `SQLite` backup integrity coverage.

#[path = "common/space_fixture.rs"]
mod space_fixture;
#[path = "common/support.rs"]
mod support;

use ma2a_store::{Repository, StoreConfig};
use rusqlite::Connection;
use space_fixture::space_record;
use support::{TempState, TestResult};

#[test]
fn online_backup_reopens_with_integrity_and_committed_state() -> TestResult {
    // Given
    let state = TempState::new("backup")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&space_record()?)?;
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

#[test]
fn concurrent_backups_to_one_destination_publish_exactly_one() -> TestResult {
    // Given: two independent repositories over the same state.
    let state = TempState::new("backup-race")?;
    let config = StoreConfig::new(state.path());
    let mut first = Repository::open(&config)?;
    first.create_space(&space_record()?)?;
    let second = Repository::open(&config)?;
    let destination = state.path().join("raced.sqlite3");

    // When
    let results = std::thread::scope(|scope| {
        let racers = [first, second].map(|repository| {
            let destination = destination.clone();
            scope.spawn(move || repository.backup_to(&destination).is_ok())
        });
        racers.map(|racer| racer.join().unwrap_or(false))
    });

    // Then: one wins, its backup is complete, and no staging file is left.
    assert_eq!(results.iter().filter(|won| **won).count(), 1);
    let backup = Connection::open(&destination)?;
    assert_eq!(
        backup.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    let leftovers = std::fs::read_dir(state.path())?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".ma2a-backup-")
        })
        .count();
    assert_eq!(leftovers, 0);
    Ok(())
}

#[test]
fn an_existing_destination_is_left_untouched() -> TestResult {
    let state = TempState::new("backup-existing")?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    let destination = state.path().join("taken.sqlite3");
    std::fs::write(&destination, b"someone else's file")?;

    assert!(repository.backup_to(&destination).is_err());

    assert_eq!(std::fs::read(&destination)?, b"someone else's file");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_backup_is_owner_only() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let state = TempState::new("backup-mode")?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    let destination = state.path().join("private.sqlite3");

    repository.backup_to(&destination)?;

    assert_eq!(
        std::fs::metadata(&destination)?.permissions().mode() & 0o777,
        0o600
    );
    Ok(())
}

#[test]
fn a_destination_name_near_the_component_limit_is_backed_up() -> TestResult {
    // Given: a destination whose name leaves no room for a longer staging name.
    let state = TempState::new("backup-long-name")?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    let destination = state.path().join(format!("{}.sqlite", "b".repeat(240)));

    // When
    repository.backup_to(&destination)?;

    // Then: it is published and reopens, with no staging entry left beside it.
    assert!(destination.is_file());
    drop(rusqlite::Connection::open(&destination)?);
    let staged = std::fs::read_dir(state.path())?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".ma2a-backup-")
        })
        .count();
    assert_eq!(staged, 0);
    Ok(())
}
