#[path = "mutation_replay_models.rs"]
mod models;

use ma2a_core::RequestId;
use rusqlite::OptionalExtension as _;

use crate::{Repository, StoreError};
pub use models::{
    LOCAL_MUTATION_REPLAY_MAX_BYTES, LOCAL_MUTATION_REPLAY_MAX_ENTRIES,
    LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES, MutationReplayRecord, MutationReplayRequest,
    MutationReplayState,
};

impl Repository {
    /// Loads one retained local mutation decision.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `SQLite` fails or the retained row is malformed.
    pub fn mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<Option<MutationReplayState>, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT fingerprint, revision, response, 0 AS retryable FROM local_mutation_replay
                 WHERE request_id = ?1
                 UNION ALL
                 SELECT fingerprint, NULL, NULL, retryable FROM local_mutation_fences
                 WHERE request_id = ?1 AND NOT EXISTS (
                    SELECT 1 FROM local_mutation_replay WHERE request_id = ?1
                 )",
                [request_id.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<u64>>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, u8>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map_or(Ok(None), |(fingerprint, revision, response, retryable)| {
            let fingerprint =
                <[u8; 32]>::try_from(fingerprint).map_err(|_| StoreError::SchemaMismatch {
                    detail: "local mutation replay fingerprint is malformed",
                })?;
            match (revision, response, retryable) {
                (None, None, 0) => Ok(Some(MutationReplayState::Pending(fingerprint))),
                (None, None, 1) => Ok(Some(MutationReplayState::Retryable(fingerprint))),
                (Some(revision), Some(response), 0) => {
                    Self::validate_replay_response(&response)?;
                    Ok(Some(MutationReplayState::Completed(MutationReplayRecord {
                        request_id,
                        fingerprint,
                        revision,
                        response,
                    })))
                }
                _ => Err(StoreError::SchemaMismatch {
                    detail: "local mutation replay completion is inconsistent",
                }),
            }
        })
    }

    /// Durably reserves one request before any mutation or external side effect begins.
    ///
    /// # Errors
    /// Returns [`StoreError`] when capacity cannot be reclaimed or `SQLite` rejects the transaction.
    pub fn reserve_mutation_replay(
        &mut self,
        request_id: RequestId,
        fingerprint: [u8; 32],
    ) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let sequence = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM local_mutation_replay",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        transaction.execute(
            "INSERT INTO local_mutation_replay(
                request_id, fingerprint, revision, response, sequence
             ) VALUES (?1, ?2, NULL, NULL, ?3)",
            (
                request_id.as_bytes().as_slice(),
                fingerprint.as_slice(),
                sequence,
            ),
        )?;
        enforce_retention(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    /// Removes an in-flight reservation after a known no-effect refusal.
    ///
    /// A fence already marked retryable after an earlier uncertain remote outcome stays bound.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `SQLite` rejects the deletion.
    pub fn abort_mutation_replay(&mut self, request_id: RequestId) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "DELETE FROM local_mutation_fences WHERE request_id = ?1 AND retryable = 0 AND EXISTS (
                SELECT 1 FROM local_mutation_replay WHERE request_id = ?1 AND response IS NULL
            )",
            [request_id.as_bytes().as_slice()],
        )?;
        transaction.execute(
            "DELETE FROM local_mutation_replay
             WHERE request_id = ?1 AND response IS NULL",
            [request_id.as_bytes().as_slice()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Transitions an in-flight reservation into a retryable fence after an uncertain remote outcome.
    ///
    /// # Errors
    /// Returns [`StoreError`] when no pending reservation exists or `SQLite` rejects the update.
    pub fn defer_mutation_replay(&mut self, request_id: RequestId) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE local_mutation_fences SET retryable = 1
             WHERE request_id = ?1 AND EXISTS (
                 SELECT 1 FROM local_mutation_replay WHERE request_id = ?1 AND response IS NULL
             )",
            [request_id.as_bytes().as_slice()],
        )?;
        if changed != 1 {
            return Err(StoreError::SchemaMismatch {
                detail: "local mutation replay reservation is missing",
            });
        }
        transaction.execute(
            "DELETE FROM local_mutation_replay WHERE request_id = ?1 AND response IS NULL",
            [request_id.as_bytes().as_slice()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Frees the bounded slots of reservations that no live process can settle anymore.
    ///
    /// Only the Runtime that owns this state directory may call this, at start, before it
    /// admits anything. The permanent fence of each request stays, so a non-retryable request is
    /// still answered as pending and a retryable request stays fenced for exact retry.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `SQLite` rejects the deletion.
    pub fn reclaim_abandoned_mutation_replays(&mut self) -> Result<usize, StoreError> {
        let transaction = self.immediate()?;
        let reclaimed = transaction.execute(
            "DELETE FROM local_mutation_replay WHERE response IS NULL AND EXISTS (
                SELECT 1 FROM local_mutation_fences AS fence
                WHERE fence.request_id = local_mutation_replay.request_id
                AND fence.fingerprint = local_mutation_replay.fingerprint
            )",
            [],
        )?;
        transaction.commit()?;
        Ok(reclaimed)
    }

    /// Stores one decision and evicts oldest rows until both retention budgets hold.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the record conflicts or the transaction cannot commit.
    pub fn record_mutation_replay(
        &mut self,
        record: &MutationReplayRecord,
    ) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE local_mutation_replay
             SET revision = ?2, response = ?3
             WHERE request_id = ?1 AND fingerprint = ?4 AND response IS NULL",
            (
                record.request_id.as_bytes().as_slice(),
                record.revision,
                record.response.as_slice(),
                record.fingerprint.as_slice(),
            ),
        )?;
        if changed != 1 {
            return Err(StoreError::SchemaMismatch {
                detail: "local mutation replay reservation is missing",
            });
        }
        transaction.execute(
            "UPDATE local_mutation_fences SET retryable = 0
             WHERE request_id = ?1 AND fingerprint = ?2",
            (
                record.request_id.as_bytes().as_slice(),
                record.fingerprint.as_slice(),
            ),
        )?;
        enforce_retention(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    const fn validate_replay_response(response: &[u8]) -> Result<(), StoreError> {
        if response.is_empty() || response.len() > LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES {
            Err(StoreError::SchemaMismatch {
                detail: "local mutation replay response is malformed",
            })
        } else {
            Ok(())
        }
    }
}

fn enforce_retention(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    loop {
        let (count, bytes) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(response)), 0)
             FROM local_mutation_replay",
            [],
            |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
        )?;
        if count <= LOCAL_MUTATION_REPLAY_MAX_ENTRIES && bytes <= LOCAL_MUTATION_REPLAY_MAX_BYTES {
            return Ok(());
        }
        let deleted = transaction.execute(
            "DELETE FROM local_mutation_replay WHERE request_id = (
                SELECT request_id FROM local_mutation_replay
                WHERE response IS NOT NULL
                ORDER BY sequence ASC, request_id ASC LIMIT 1
             )",
            [],
        )?;
        if deleted != 1 {
            return Err(StoreError::SchemaMismatch {
                detail: "local mutation replay capacity is exhausted by pending requests",
            });
        }
    }
}
