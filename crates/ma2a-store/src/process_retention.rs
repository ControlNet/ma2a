//! Bounded retention of ended Processes and their tombstones.

use ma2a_core::service::ProcessId;
use rusqlite::params;

use crate::{
    Committed, ProcessArchive, Repository, StoreError, processes::load,
    repository::increment_revision,
};

/// How long an ended Process keeps its output before it is archived: 7 days.
pub const ENDED_RETENTION_MS: u64 = 7 * 86_400_000;
/// How long an archived tombstone keeps its `RequestId` binding: 30 days.
pub const TOMBSTONE_RETENTION_MS: u64 = 30 * 86_400_000;
/// Most archived tombstones kept; the oldest are pruned first.
pub const MAX_TOMBSTONES: u32 = 1_024;

impl Repository {
    /// Archives Processes that ended over [`ENDED_RETENTION_MS`] ago and prunes expired or
    /// excess tombstones. Returns the Processes archived now, whose output may be removed.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn expire_processes(&mut self, now_ms: u64) -> Result<Vec<ProcessId>, StoreError> {
        let transaction = self.immediate()?;
        let ended_before = now_ms.saturating_sub(ENDED_RETENTION_MS);
        let archived = {
            let mut statement = transaction.prepare(
                "UPDATE processes SET start = NULL, archived_at_ms = ?1
                 WHERE state BETWEEN 3 AND 6 AND archived_at_ms IS NULL AND ended_at_ms < ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_instances
                       WHERE process_id = processes.process_id AND ended_at_ms IS NULL
                   )
                 RETURNING process_id",
            )?;
            let rows = statement.query_map(params![now_ms, ended_before], |row| {
                row.get::<_, Vec<u8>>(0)
            })?;
            rows.map(|row| {
                let bytes = row?;
                <[u8; 16]>::try_from(bytes.as_slice())
                    .map(ProcessId::from_bytes)
                    .map_err(|_| StoreError::SchemaMismatch {
                        detail: "persisted Process is malformed",
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
        };
        let pruned = transaction.execute(
            "DELETE FROM processes WHERE archived_at_ms IS NOT NULL AND (archived_at_ms < ?1
             OR process_id NOT IN (SELECT process_id FROM processes
                 WHERE archived_at_ms IS NOT NULL
                 ORDER BY archived_at_ms DESC, process_id DESC LIMIT ?2))",
            params![
                now_ms.saturating_sub(TOMBSTONE_RETENTION_MS),
                MAX_TOMBSTONES
            ],
        )?;
        if !archived.is_empty() || pruned > 0 {
            increment_revision(&transaction)?;
        }
        transaction.commit()?;
        Ok(archived)
    }

    /// Archives one terminal Process: its start is removed and a tombstone remains.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn archive_process(
        &mut self,
        id: ProcessId,
        at_ms: u64,
    ) -> Result<Committed<ProcessArchive>, StoreError> {
        let transaction = self.immediate()?;
        let outcome = match load(&transaction, id)? {
            None => ProcessArchive::Missing,
            Some(record) if !record.state.is_terminal() => ProcessArchive::Active,
            Some(record) if record.is_archived() => ProcessArchive::Archived(Box::new(record)),
            Some(_) => {
                let bound_to_live_instance = transaction.query_row(
                    "SELECT EXISTS (SELECT 1 FROM agent_instances
                     WHERE process_id = ?1 AND ended_at_ms IS NULL)",
                    [id.as_bytes().as_slice()],
                    |row| row.get::<_, bool>(0),
                )?;
                if bound_to_live_instance {
                    ProcessArchive::Active
                } else {
                    transaction.execute(
                        "UPDATE processes SET start = NULL, archived_at_ms = ?2 WHERE process_id = ?1",
                        params![id.as_bytes().as_slice(), at_ms],
                    )?;
                    increment_revision(&transaction)?;
                    ProcessArchive::Archived(Box::new(load(&transaction, id)?.ok_or(
                        StoreError::SchemaMismatch {
                            detail: "an archived Process could not be read back",
                        },
                    )?))
                }
            }
        };
        let revision = crate::service_exposures::current_revision(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, outcome))
    }
}
