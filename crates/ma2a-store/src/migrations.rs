use rusqlite::{Connection, TransactionBehavior};

use crate::StoreError;

/// The only schema version this Phase 1 store can read or write.
pub const SCHEMA_VERSION: u32 = 1;
const MIGRATION_V1: &str = include_str!("../migrations/0001_init.sql");

pub(crate) fn current_version(connection: &Connection) -> Result<u32, StoreError> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let version = current_version(connection)?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::FutureSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == 0 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(MIGRATION_V1)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
    }
    validate(connection)
}

fn validate(connection: &Connection) -> Result<(), StoreError> {
    if current_version(connection)? != SCHEMA_VERSION {
        return Err(StoreError::SchemaMismatch {
            detail: "user_version does not equal schema v1",
        });
    }
    let migration_count = connection.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1 AND name = 'initial'",
        [SCHEMA_VERSION],
        |row| row.get::<_, u32>(0),
    )?;
    if migration_count != 1 {
        return Err(StoreError::SchemaMismatch {
            detail: "schema migration record is missing",
        });
    }
    let integrity =
        connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    if integrity != "ok" {
        return Err(StoreError::SchemaMismatch {
            detail: "SQLite quick_check failed",
        });
    }
    Ok(())
}
