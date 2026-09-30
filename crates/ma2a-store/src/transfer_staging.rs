//! Durable upload staging records written before a staging file is created.

use ma2a_core::RequestId;
use rusqlite::params;

use crate::{Repository, StoreError};

/// One upload staging file, recorded before the file is created.
#[derive(Clone, Copy, Debug)]
pub struct TransferStaging<'a> {
    staging_path: &'a str,
    request_id: RequestId,
    created_at_ms: u64,
}

impl<'a> TransferStaging<'a> {
    /// Describes the staging file of one upload request.
    pub const fn new(staging_path: &'a str, request_id: RequestId, created_at_ms: u64) -> Self {
        Self {
            staging_path,
            request_id,
            created_at_ms,
        }
    }
}

impl Repository {
    /// Records a staging path before the file is created, so it can always be cleaned up.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the record cannot be committed.
    pub fn record_transfer_staging(
        &mut self,
        staging: TransferStaging<'_>,
    ) -> Result<(), StoreError> {
        let TransferStaging {
            staging_path,
            request_id,
            created_at_ms,
        } = staging;
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO transfer_staging(staging_path, request_id, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![
                staging_path,
                request_id.as_bytes().as_slice(),
                created_at_ms
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Forgets a staging path after its file was finalized or removed.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the record cannot be removed.
    pub fn remove_transfer_staging(&mut self, staging_path: &str) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "DELETE FROM transfer_staging WHERE staging_path = ?1",
            [staging_path],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Lists every recorded staging path, oldest first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the records cannot be read.
    pub fn transfer_staging_paths(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT staging_path FROM transfer_staging ORDER BY created_at_ms, staging_path",
        )?;
        let paths = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(paths)
    }
}
