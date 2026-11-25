use ma2a_core::service::{AgentEvent, AgentEventBody};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::StoreError;

/// The newest schema version this store can read or write.
pub const SCHEMA_VERSION: u32 = 13;
const MIGRATION_V1: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_web_auth.sql");
const MIGRATION_V3: &str = include_str!("../migrations/0003_relay_persistence.sql");
const MIGRATION_V4: &str = include_str!("../migrations/0004_local_mutation_replay.sql");
const MIGRATION_V5: &str = include_str!("../migrations/0005_relay_advertisement_activity.sql");

const MIGRATION_V6: &str = include_str!("../migrations/0006_mutation_fences.sql");
const MIGRATION_V7: &str = include_str!("../migrations/0007_service_exposures.sql");
const MIGRATION_V8: &str = include_str!("../migrations/0008_processes.sql");
const MIGRATION_V9: &str = include_str!("../migrations/0009_agents.sql");
const MIGRATION_V10: &str = include_str!("../migrations/0010_hosted_services.sql");
const MIGRATION_V11: &str = include_str!("../migrations/0011_departure_receipts.sql");
const MIGRATION_V12: &str = include_str!("../migrations/0012_agent_task_changed.sql");
const MIGRATION_V13: &str = include_str!("../migrations/0013_agent_outbox_task_fk.sql");

pub(crate) fn current_version(connection: &Connection) -> Result<u32, StoreError> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    // Current-schema validation only reads. Taking a writer reservation here
    // makes harmless repository opens (including session validation) compete
    // with actual mutations and fail with SQLITE_BUSY under ordinary overlap.
    // A real migration still serializes on IMMEDIATE and rechecks the version.
    let behavior = if current_version(connection)? == SCHEMA_VERSION {
        TransactionBehavior::Deferred
    } else {
        TransactionBehavior::Immediate
    };
    let transaction = connection.transaction_with_behavior(behavior)?;
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
        transaction.pragma_update(None, "user_version", 3_u32)?;
    }
    if current_version(&transaction)? == 3 {
        transaction.execute_batch(MIGRATION_V4)?;
        transaction.pragma_update(None, "user_version", 4_u32)?;
    }
    if current_version(&transaction)? == 4 {
        transaction.execute_batch(MIGRATION_V5)?;
        transaction.pragma_update(None, "user_version", 5_u32)?;
    }
    if current_version(&transaction)? == 5 {
        transaction.execute_batch(MIGRATION_V6)?;
        transaction.pragma_update(None, "user_version", 6_u32)?;
    }
    if current_version(&transaction)? == 6 {
        transaction.execute_batch(MIGRATION_V7)?;
        transaction.pragma_update(None, "user_version", 7_u32)?;
    }
    if current_version(&transaction)? == 7 {
        transaction.execute_batch(MIGRATION_V8)?;
        transaction.pragma_update(None, "user_version", 8_u32)?;
    }
    if current_version(&transaction)? == 8 {
        transaction.execute_batch(MIGRATION_V9)?;
        transaction.pragma_update(None, "user_version", 9_u32)?;
    }
    if current_version(&transaction)? == 9 {
        transaction.execute_batch(MIGRATION_V10)?;
        transaction.pragma_update(None, "user_version", 10_u32)?;
    }
    if current_version(&transaction)? == 10 {
        transaction.execute_batch(MIGRATION_V11)?;
        transaction.pragma_update(None, "user_version", 11_u32)?;
    }
    if current_version(&transaction)? == 11 {
        transaction.execute_batch(MIGRATION_V12)?;
        backfill_task_changed_at(&transaction)?;
        transaction.pragma_update(None, "user_version", 12_u32)?;
    }
    if current_version(&transaction)? == 12 {
        transaction.execute_batch(MIGRATION_V13)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    validate(&transaction)?;
    transaction.commit()?;
    Ok(())
}

/// Refines `agent_tasks.changed_at_ms` from each task's newest retained `state` event;
/// tasks without a retained state event keep the SQL fallback.
fn backfill_task_changed_at(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    let mut updates = Vec::new();
    {
        let mut statement = transaction.prepare(
            "SELECT task_id, event FROM agent_task_events WHERE progress = 0
             ORDER BY task_id, seq DESC",
        )?;
        let mut rows = statement.query([])?;
        let mut seen: Option<Vec<u8>> = None;
        while let Some(row) = rows.next()? {
            let task_id: Vec<u8> = row.get(0)?;
            if seen.as_deref() == Some(task_id.as_slice()) {
                continue;
            }
            let raw: Vec<u8> = row.get(1)?;
            let event = AgentEvent::from_bytes(&raw)?;
            if matches!(event.body, AgentEventBody::State(..)) {
                seen = Some(task_id.clone());
                updates.push((task_id, event.at_ms));
            }
        }
    }
    let mut update =
        transaction.prepare("UPDATE agent_tasks SET changed_at_ms = ?2 WHERE task_id = ?1")?;
    for (task_id, at_ms) in updates {
        update.execute(params![task_id, at_ms])?;
    }
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
         OR (version = 3 AND name = 'relay_persistence')
         OR (version = 4 AND name = 'local_mutation_replay')
         OR (version = 5 AND name = 'relay_advertisement_activity')
         OR (version = 6 AND name = 'mutation_fences')
         OR (version = 7 AND name = 'service_exposures')
         OR (version = 8 AND name = 'processes')
         OR (version = 9 AND name = 'agents')
         OR (version = 10 AND name = 'hosted_services')
         OR (version = 11 AND name = 'departure_receipts')
         OR (version = 12 AND name = 'agent_task_changed')
         OR (version = 13 AND name = 'agent_outbox_task_fk')",
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
