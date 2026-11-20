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
    /// The backup is written to a new file inside a new private directory beside `destination`,
    /// checked, and only then published under `destination` by a hard link that never replaces
    /// anything. Concurrent backups to one destination therefore publish exactly one complete
    /// backup, and a failed backup never leaves `destination` behind.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the destination exists, backup I/O fails, the file system
    /// cannot create hard links, the staged file was replaced, or the completed backup fails its
    /// integrity check.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(already_exists());
        }
        let staging = Staging::create(destination)?;
        let written = self
            .write_backup(&staging.file)
            .and_then(|()| staging.publish(destination));
        staging.remove();
        written
    }

    fn write_backup(&self, staged: &Path) -> Result<(), StoreError> {
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

const fn replaced() -> StoreError {
    StoreError::SchemaMismatch {
        detail: "the staged backup was replaced before it was published",
    }
}

/// The one staged backup file, inside a fresh private directory beside the destination.
///
/// The directory has a short random name, whatever the destination is called, so publishing is
/// a same-directory hard link. Only this account may enter it, and the file created in it is
/// held open, so publishing can confirm that a name still refers to that very file.
struct Staging {
    directory: PathBuf,
    file: PathBuf,
    handle: File,
}

impl Staging {
    fn create(destination: &Path) -> Result<Self, StoreError> {
        let mut name = String::from(".ma2a-backup-");
        for byte in ma2a_core::RequestId::random()?.as_bytes() {
            let _written = write!(name, "{byte:02x}");
        }
        let directory = destination.with_file_name(name);
        permissions::create_new_private_dir(&directory)?;
        let file = directory.join("backup.sqlite");
        match create_private(&file) {
            Ok(handle) => Ok(Self {
                directory,
                file,
                handle,
            }),
            Err(error) => {
                let _removed = fs::remove_dir(&directory);
                Err(error)
            }
        }
    }

    /// Publishes the checked backup under `destination` without replacing an existing file,
    /// and only when both names refer to the file this backup created.
    fn publish(&self, destination: &Path) -> Result<(), StoreError> {
        if !self.holds(&self.file)? {
            return Err(replaced());
        }
        match fs::hard_link(&self.file, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(already_exists());
            }
            // No fallback: a rename could replace a file another backup published meanwhile.
            Err(error) => return Err(error.into()),
        }
        if self.holds(destination)? {
            Ok(())
        } else {
            // Whatever is there now may not be ours to remove; report it instead.
            Err(replaced())
        }
    }

    /// Whether `path` names the file this backup created, without following a link.
    #[cfg(unix)]
    fn holds(&self, path: &Path) -> Result<bool, StoreError> {
        use std::os::unix::fs::MetadataExt as _;
        let (held, named) = (self.handle.metadata()?, fs::symlink_metadata(path)?);
        Ok(named.is_file() && (held.dev(), held.ino()) == (named.dev(), named.ino()))
    }

    /// Windows offers no stable file identity here; the directory's ACL is the protection.
    #[cfg(not(unix))]
    fn holds(&self, path: &Path) -> Result<bool, StoreError> {
        let _held = &self.handle;
        Ok(fs::symlink_metadata(path)?.is_file())
    }

    /// Removes the staging file and directory; the published backup keeps its own link.
    fn remove(self) {
        drop(self.handle);
        let _removed = fs::remove_file(&self.file);
        let _removed = fs::remove_dir(&self.directory);
    }
}

/// Creates `path` exclusively; on Unix it is owner-only from the moment it exists.
fn create_private(path: &Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    Ok(options.open(path)?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::Staging;

    #[test]
    fn a_replaced_staging_file_is_never_published() -> Result<(), Box<dyn std::error::Error>> {
        // Given: a staged backup whose name now refers to another file.
        let scratch = std::env::temp_dir().join(format!("ma2a-backup-swap-{}", std::process::id()));
        std::fs::create_dir_all(&scratch)?;
        let destination = scratch.join("state.sqlite");
        let staging = Staging::create(&destination)?;
        std::fs::remove_file(&staging.file)?;
        std::fs::write(&staging.file, b"not the backup")?;

        // When
        let published = staging.publish(&destination);

        // Then: nothing is published, and cleanup leaves no staging directory.
        assert!(published.is_err());
        assert!(!destination.exists());
        staging.remove();
        assert_eq!(std::fs::read_dir(&scratch)?.count(), 0);
        std::fs::remove_dir(&scratch)?;
        Ok(())
    }
}
