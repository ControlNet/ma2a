use ma2a_core::RequestId;
use rusqlite::OptionalExtension as _;

use crate::{Repository, StoreError};

/// Maximum terminal local mutation decisions retained across restarts.
pub const LOCAL_MUTATION_REPLAY_MAX_ENTRIES: usize = 1_024;
/// Maximum aggregate encoded response bytes retained across restarts.
pub const LOCAL_MUTATION_REPLAY_MAX_BYTES: usize = 8 * 1_024 * 1_024;
/// Maximum encoded response bytes retained for one terminal mutation.
pub const LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES: usize = 65_536;

/// One bounded terminal local mutation decision stored without command payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationReplayRecord {
    request_id: RequestId,
    fingerprint: [u8; 32],
    revision: u64,
    response: Vec<u8>,
}

/// Correlation identity and canonical fingerprint for one local mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationReplayRequest {
    request_id: RequestId,
    fingerprint: [u8; 32],
}

/// Durable classification for one local mutation request identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MutationReplayState {
    /// The request was durably admitted but no retained terminal response is available.
    Pending([u8; 32]),
    /// The request completed and its exact terminal response is retained.
    Completed(MutationReplayRecord),
}

impl MutationReplayRequest {
    /// Creates one replay request identity from validated typed command data.
    pub const fn new(request_id: RequestId, fingerprint: [u8; 32]) -> Self {
        Self {
            request_id,
            fingerprint,
        }
    }

    /// Returns the mutation correlation identifier.
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    /// Returns the canonical typed-command fingerprint.
    pub const fn fingerprint(self) -> [u8; 32] {
        self.fingerprint
    }
}

impl MutationReplayRecord {
    /// Creates one validated replay record from an already bounded terminal response.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the response is empty or exceeds the per-entry bound.
    pub fn new(
        request: MutationReplayRequest,
        revision: u64,
        response: Vec<u8>,
    ) -> Result<Self, StoreError> {
        if response.is_empty() || response.len() > LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES {
            return Err(StoreError::SchemaMismatch {
                detail: "local mutation replay response is outside its bound",
            });
        }
        Ok(Self {
            request_id: request.request_id,
            fingerprint: request.fingerprint,
            revision,
            response,
        })
    }

    /// Returns the mutation correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the canonical typed-command fingerprint.
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Returns the original committed response revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the exact encoded terminal response.
    pub fn response(&self) -> &[u8] {
        &self.response
    }
}

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
                "SELECT fingerprint, revision, response FROM local_mutation_replay
                 WHERE request_id = ?1
                 UNION ALL
                 SELECT fingerprint, NULL, NULL FROM local_mutation_fences
                 WHERE request_id = ?1 AND NOT EXISTS (
                    SELECT 1 FROM local_mutation_replay WHERE request_id = ?1
                 )",
                [request_id.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<u64>>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map_or(Ok(None), |(fingerprint, revision, response)| {
            let fingerprint =
                <[u8; 32]>::try_from(fingerprint).map_err(|_| StoreError::SchemaMismatch {
                    detail: "local mutation replay fingerprint is malformed",
                })?;
            match (revision, response) {
                (None, None) => Ok(Some(MutationReplayState::Pending(fingerprint))),
                (Some(revision), Some(response)) => {
                    Self::validate_replay_response(&response)?;
                    Ok(Some(MutationReplayState::Completed(MutationReplayRecord {
                        request_id,
                        fingerprint,
                        revision,
                        response,
                    })))
                }
                (None, Some(_)) | (Some(_), None) => Err(StoreError::SchemaMismatch {
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

    /// Removes a reservation after execution fails without a terminal side effect.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `SQLite` rejects the deletion.
    pub fn abort_mutation_replay(&mut self, request_id: RequestId) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "DELETE FROM local_mutation_fences WHERE request_id = ?1 AND EXISTS (
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
