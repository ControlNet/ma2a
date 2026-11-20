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
    /// Returns [`StoreError`] when the destination exists, its directory may be changed by
    /// another account, backup I/O fails, the file system cannot create hard links, the staged
    /// file was replaced, or the completed backup fails its integrity check.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        // Every later step uses the path that was checked, never the caller's spelling: a
        // link on the caller's path could be pointed elsewhere once the check has passed.
        #[cfg(unix)]
        let destination = &trusted_destination(destination)?;
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
        Ok(self.handle.metadata()?.is_file() && fs::symlink_metadata(path)?.is_file())
    }

    /// Removes the staging file, only while its name still refers to the file this backup
    /// created, then the directory once it is empty; the published backup keeps its own link.
    fn remove(self) {
        if self.holds(&self.file).unwrap_or(false) {
            let _removed = fs::remove_file(&self.file);
        }
        drop(self.handle);
        let _removed = fs::remove_dir(&self.directory);
    }
}

/// Returns `destination` below its resolved directory, refusing it when that directory, or
/// any directory above it, another account could rearrange.
///
/// The staging directory is private, so only an account able to rename entries along the
/// destination's path could swap it, or the file in it, for another between the checks above:
/// renaming any ancestor swaps the whole subtree below it. On Unix every directory from the
/// root down to the destination's, after resolving links, must belong to this account or
/// root, and must not let another account rename entries: no group or other write bit unless
/// it is sticky. The resolved path holds no links, and nobody else may rename any directory on
/// it, so it keeps naming the checked directory. On Windows the directories' ACLs are the
/// operator's precondition.
#[cfg(unix)]
fn trusted_destination(destination: &Path) -> Result<PathBuf, StoreError> {
    use std::os::unix::fs::MetadataExt as _;

    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = destination.file_name().ok_or(StoreError::SchemaMismatch {
        detail: "backup destination has no file name",
    })?;
    let resolved = fs::canonicalize(parent)?;
    let euid = rustix::process::geteuid().as_raw();
    for directory in resolved.ancestors() {
        let metadata = fs::metadata(directory)?;
        let mode = metadata.mode();
        let owner = metadata.uid() == euid || metadata.uid() == 0;
        if !owner || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
            return Err(StoreError::InsecurePermissions {
                target: "backup directory",
                detail: "another account may rename entries along the backup's path",
            });
        }
    }
    Ok(resolved.join(name))
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
    use std::os::unix::fs::PermissionsExt as _;

    use super::{Staging, trusted_destination};

    #[test]
    fn a_backup_through_a_link_is_written_where_the_link_pointed_when_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        // Given: a destination reached through a directory link.
        let scratch = std::env::temp_dir().join(format!("ma2a-backup-link-{}", std::process::id()));
        let target = scratch.join("target");
        std::fs::create_dir_all(&target)?;
        for directory in [&scratch, &target] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
        std::os::unix::fs::symlink(&target, scratch.join("alias"))?;

        // When
        let resolved = trusted_destination(&scratch.join("alias").join("state.sqlite"))?;

        // Then: later steps use a path with no link left to repoint.
        assert_eq!(
            resolved,
            std::fs::canonicalize(&target)?.join("state.sqlite")
        );
        std::fs::remove_dir_all(&scratch)?;
        Ok(())
    }

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

        // Then: nothing is published, and cleanup leaves the file it did not create.
        assert!(published.is_err());
        assert!(!destination.exists());
        let replacement = staging.file.clone();
        staging.remove();
        assert_eq!(std::fs::read(&replacement)?, b"not the backup");
        std::fs::remove_dir_all(&scratch)?;
        Ok(())
    }
}
