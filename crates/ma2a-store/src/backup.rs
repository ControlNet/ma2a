use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, backup::Backup};

use crate::{Repository, StoreError, permissions};

impl Repository {
    /// Creates a consistent online `SQLite` backup while the source remains open.
    ///
    /// The backup is written to a new, owner-only file beside `destination`, checked, and only
    /// then published under `destination` without replacing anything. Concurrent backups to one
    /// destination therefore publish exactly one complete backup, and a failed backup never
    /// leaves `destination` behind.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the destination exists, backup I/O fails, or
    /// the completed backup fails its integrity check.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(already_exists());
        }
        let staged = staging_path(destination)?;
        let file = create_private(&staged)?;
        let written = self
            .write_backup(&staged, file)
            .and_then(|()| publish(&staged, destination));
        if written.is_err() {
            let _removed = fs::remove_file(&staged);
        }
        written
    }

    fn write_backup(&self, staged: &Path, file: File) -> Result<(), StoreError> {
        drop(file);
        permissions::protect_new_file(staged, "SQLite backup")?;
        let mut destination_connection = Connection::open(staged)?;
        let backup = Backup::new(&self.connection, &mut destination_connection)?;
        backup.run_to_completion(128, Duration::from_millis(1), None)?;
        drop(backup);
        let integrity = destination_connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
        destination_connection
            .close()
            .map_err(|(_, error)| StoreError::from(error))?;
        if integrity != "ok" {
            return Err(StoreError::SchemaMismatch {
                detail: "online backup integrity_check failed",
            });
        }
        Ok(())
    }
}

const fn already_exists() -> StoreError {
    StoreError::SchemaMismatch {
        detail: "backup destination already exists",
    }
}

/// A fresh name beside `destination`, so publishing is a same-directory link or rename.
fn staging_path(destination: &Path) -> Result<PathBuf, StoreError> {
    let name = destination
        .file_name()
        .ok_or(StoreError::InvalidStateDirectory)?
        .to_string_lossy();
    let mut suffix = String::new();
    for byte in ma2a_core::RequestId::random()?.as_bytes() {
        let _written = write!(suffix, "{byte:02x}");
    }
    Ok(destination.with_file_name(format!(".{name}.ma2a-backup-{suffix}")))
}

/// Creates `path` exclusively; on Unix it is owner-only from the moment it exists.
fn create_private(path: &Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    Ok(options.open(path)?)
}

/// Publishes the checked backup under `destination` without replacing an existing file.
fn publish(staged: &Path, destination: &Path) -> Result<(), StoreError> {
    match fs::hard_link(staged, destination) {
        Ok(()) => {
            // The backup is published; a staging name left behind is only clutter.
            let _removed = fs::remove_file(staged);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(already_exists()),
        // A file system without hard links falls back to a check before the rename.
        Err(_) if destination.exists() => Err(already_exists()),
        Err(_) => Ok(fs::rename(staged, destination)?),
    }
}
