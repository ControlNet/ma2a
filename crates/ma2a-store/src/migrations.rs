use rusqlite::{Connection, TransactionBehavior};

use crate::StoreError;

/// The newest schema version this Phase 1 store can read or write.
pub const SCHEMA_VERSION: u32 = 3;
const MIGRATION_V1: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_web_auth.sql");
const MIGRATION_V3: &str = include_str!("../migrations/0003_relay_persistence.sql");

pub(crate) fn current_version(connection: &Connection) -> Result<u32, StoreError> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = current_version(&transaction)?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::FutureSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == 0 {
        transaction.execute_batch(MIGRATION_V1)?;
        transaction.pragma_update(None, "user_version", 1_u32)?;
    }
    if current_version(&transaction)? == 1 {
        transaction.execute_batch(MIGRATION_V2)?;
        transaction.pragma_update(None, "user_version", 2_u32)?;
    }
    if current_version(&transaction)? == 2 {
        transaction.execute_batch(MIGRATION_V3)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    validate(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate(connection: &Connection) -> Result<(), StoreError> {
    if current_version(connection)? != SCHEMA_VERSION {
        return Err(StoreError::SchemaMismatch {
            detail: "user_version does not equal the current schema",
        });
    }
    let migration_count = connection.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE (version = 1 AND name = 'initial')
         OR (version = 2 AND name = 'web_auth')
         OR (version = 3 AND name = 'relay_persistence')",
        [],
        |row| row.get::<_, u32>(0),
    )?;
    if migration_count != SCHEMA_VERSION {
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
