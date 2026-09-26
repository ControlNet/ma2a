//! Opening an already migrated WAL store must not contend for a writer lock.
#[path = "common/support.rs"]
mod support;

use ma2a_store::{Repository, StoreConfig, StoreError};
use rusqlite::{Connection, TransactionBehavior};
use support::{TempState, TestResult};

#[test]
fn current_schema_open_reads_committed_truth_while_another_writer_is_active() -> TestResult {
    let state = TempState::new("open-during-write")?;
    let config = StoreConfig::new(state.path());
    let revision = Repository::open(&config)?.revision()?;
    let mut writer = Connection::open(config.database_path())?;
    let transaction = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute("UPDATE runtime_metadata SET revision = revision + 1", [])?;
    // The write lock is deliberately held until open finishes; no scheduling race.
    let reader = Repository::open(&config)?;
    assert_eq!(reader.revision()?, revision);
    transaction.commit()?;
    assert_eq!(reader.revision()?, revision + 1);
    Ok(())
}

#[test]
fn current_schema_read_path_still_rejects_missing_migration_evidence() -> TestResult {
    let state = TempState::new("invalid-current-schema")?;
    let config = StoreConfig::new(state.path());
    drop(Repository::open(&config)?);
    let sql = Connection::open(config.database_path())?;
    sql.execute("DELETE FROM schema_migrations WHERE version = 1", [])?;
    assert!(matches!(
        Repository::open(&config),
        Err(StoreError::SchemaMismatch { .. })
    ));
    Ok(())
}
