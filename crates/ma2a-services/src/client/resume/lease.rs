//! The exclusive lease one download holds on its destination: an OS lock on a lock file.
//!
//! The lock file is removed when the download ends. On Unix it is removed while still
//! locked, so a download that locks the removed file afterwards sees that the path names
//! another file and opens again. On Windows it is opened without delete sharing, so it
//! cannot be removed while any download still has it open.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::PathBuf,
};

/// How often a lock on a file that was removed meanwhile is retried.
const ATTEMPTS: usize = 4;

/// One held lease; dropping it without [`Lease::release`] keeps the lock file on disk.
#[derive(Debug)]
pub(super) struct Lease {
    path: PathBuf,
    file: File,
}

impl Lease {
    /// Takes the lease at `path` without waiting.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::WouldBlock`] while another download holds it, or another
    /// local file error.
    pub(super) async fn acquire(path: PathBuf) -> io::Result<Self> {
        tokio::task::spawn_blocking(move || {
            for _ in 0..ATTEMPTS {
                if let Some(file) = locked(&path)? {
                    return Ok(Self { path, file });
                }
            }
            Err(in_progress())
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Ends the lease and removes its lock file.
    pub(super) async fn release(self) {
        let Self { path, file } = self;
        let _released = tokio::task::spawn_blocking(move || {
            if cfg!(windows) {
                drop(file);
                let _removed = std::fs::remove_file(&path);
            } else {
                let _removed = std::fs::remove_file(&path);
                drop(file);
            }
        })
        .await;
    }
}

/// Opens and locks the lock file; `None` means it was removed before the lock was taken.
fn locked(path: &std::path::Path) -> io::Result<Option<File>> {
    let file = open(path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(in_progress()),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    // A lock file without delete sharing cannot be removed while open, so on Windows the
    // locked file is always the one the path names.
    #[cfg(unix)]
    if !still_named(&file, path)? {
        return Ok(None);
    }
    Ok(Some(file))
}

fn in_progress() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "another download to this destination is in progress",
    )
}

#[cfg(unix)]
fn open(path: &std::path::Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

#[cfg(windows)]
fn open(path: &std::path::Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
}

/// Returns whether `path` still names the locked `file`, not a file created after removal.
#[cfg(unix)]
fn still_named(file: &File, path: &std::path::Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let held = file.metadata()?;
    match std::fs::metadata(path) {
        Ok(named) => Ok(named.dev() == held.dev() && named.ino() == held.ino()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
