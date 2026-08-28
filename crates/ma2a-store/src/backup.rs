use std::{path::Path, time::Duration};

use rusqlite::{Connection, backup::Backup};

use crate::{Repository, StoreError, permissions};

impl Repository {
    /// Creates a consistent online `SQLite` backup while the source remains open.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the destination exists, backup I/O fails, or
    /// the completed backup fails its integrity check.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(StoreError::SchemaMismatch {
                detail: "backup destination already exists",
            });
        }
        let mut destination_connection = Connection::open(destination)?;
        permissions::protect_new_file(destination, "SQLite backup")?;
        let backup = Backup::new(&self.connection, &mut destination_connection)?;
        backup.run_to_completion(128, Duration::from_millis(1), None)?;
        drop(backup);
        let integrity = destination_connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
        if integrity != "ok" {
            return Err(StoreError::SchemaMismatch {
                detail: "online backup integrity_check failed",
            });
        }
        Ok(())
    }
}
