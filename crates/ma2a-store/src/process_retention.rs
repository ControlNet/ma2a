//! Bounded retention of ended Processes and their tombstones.

use ma2a_core::{EndpointId, service::ProcessId};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    Committed, MAX_ACTIVE_PROCESSES, MAX_ACTIVE_PROCESSES_PER_CREATOR, MAX_RETAINED_PROCESSES,
    MAX_RETAINED_PROCESSES_PER_CREATOR, NewProcess, ProcessArchive, ProcessCreation, Repository,
    StoreError,
    processes::{ACTIVE, RETAINED, count, find_by_binding, insert_process, load},
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
                    archive_row(&transaction, id, at_ms)?;
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

    /// Records a local start, atomically archiving the oldest eligible ended local Process when
    /// below live limits and a retained limit is reached.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn create_local_process(
        &mut self,
        new: &NewProcess<'_>,
    ) -> Result<Committed<(ProcessCreation, Option<ProcessId>)>, StoreError> {
        self.create_process_inner(new, true)
    }

    pub(crate) fn create_process_inner(
        &mut self,
        new: &NewProcess<'_>,
        reclaim_retained: bool,
    ) -> Result<Committed<(ProcessCreation, Option<ProcessId>)>, StoreError> {
        let transaction = self.immediate()?;
        let (creator, request_id) = new.binding;
        let existing = find_by_binding(&transaction, creator, request_id)?;
        let mut archived = None;
        let creation = if let Some(record) = existing {
            if record.fingerprint == new.fingerprint {
                ProcessCreation::Existing(record)
            } else {
                ProcessCreation::Conflict
            }
        } else if count(&transaction, ACTIVE, Some(creator))? >= MAX_ACTIVE_PROCESSES_PER_CREATOR
            || count(&transaction, ACTIVE, None)? >= MAX_ACTIVE_PROCESSES
        {
            ProcessCreation::LimitExceeded
        } else {
            let creator_retained = count(&transaction, RETAINED, Some(creator))?;
            let total_retained = count(&transaction, RETAINED, None)?;
            let needs_room = creator_retained >= MAX_RETAINED_PROCESSES_PER_CREATOR
                || total_retained >= MAX_RETAINED_PROCESSES;
            let room_made = if !needs_room {
                true
            } else if reclaim_retained
                && creator_retained <= MAX_RETAINED_PROCESSES_PER_CREATOR
                && total_retained <= MAX_RETAINED_PROCESSES
            {
                archived = reclaim_oldest_local(&transaction, creator, new.created_ms)?;
                archived.is_some()
            } else {
                false
            };
            if room_made {
                ProcessCreation::Created(insert_process(&transaction, new)?)
            } else {
                ProcessCreation::LimitExceeded
            }
        };
        let revision = crate::service_exposures::current_revision(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, (creation, archived)))
    }
}

pub(crate) fn archive_row(
    transaction: &Transaction<'_>,
    id: ProcessId,
    at_ms: u64,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE processes SET start = NULL, archived_at_ms = ?2 WHERE process_id = ?1",
        params![id.as_bytes().as_slice(), at_ms],
    )?;
    Ok(())
}

/// Archives the oldest ended Process of `creator` not bound to an unconcluded Agent instance.
pub(crate) fn reclaim_oldest_local(
    transaction: &Transaction<'_>,
    creator: EndpointId,
    at_ms: u64,
) -> Result<Option<ProcessId>, StoreError> {
    let candidate = transaction
        .query_row(
            "SELECT process_id FROM processes
             WHERE creator_endpoint_id = ?1
               AND state BETWEEN 3 AND 6
               AND archived_at_ms IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM agent_instances
                   WHERE process_id = processes.process_id AND ended_at_ms IS NULL
               )
             ORDER BY ended_at_ms ASC, process_id ASC
             LIMIT 1",
            [creator.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let Some(bytes) = candidate else {
        return Ok(None);
    };
    let id = <[u8; 16]>::try_from(bytes.as_slice())
        .map(ProcessId::from_bytes)
        .map_err(|_| StoreError::SchemaMismatch {
            detail: "persisted Process is malformed",
        })?;
    archive_row(transaction, id, at_ms)?;
    Ok(Some(id))
}
