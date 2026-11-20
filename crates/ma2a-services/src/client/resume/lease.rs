//! The exclusive lease one download holds on its destination: an OS lock on its lock file.
//!
//! The lock file is removed when the download ends. On Unix it is removed while still
//! locked, so a download that locks the removed file afterwards sees that the name now refers
//! to another file and opens again. On Windows it is opened without delete sharing, so it
//! cannot be removed while any download still has it open.

use std::{
    ffi::{OsStr, OsString},
    fs::{File, TryLockError},
    io,
};

use super::anchor::{Anchor, Open};

/// How often a lock on a file that was removed meanwhile is retried.
const ATTEMPTS: usize = 4;

/// One held lease; dropping it without [`Lease::release`] keeps the lock file on disk.
#[derive(Debug)]
pub(super) struct Lease {
    name: OsString,
    file: File,
}

impl Lease {
    /// Takes the lease on the lock file `name` in `locks` without waiting.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::WouldBlock`] while another download holds it, or another
    /// local file error.
    pub(super) fn acquire(locks: &Anchor, name: &OsStr) -> io::Result<Self> {
        for _ in 0..ATTEMPTS {
            let file = locks.open_file(name, Open::Create)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Err(in_progress()),
                Err(TryLockError::Error(error)) => return Err(error),
            }
            // A lock file without delete sharing cannot be removed while open, so on Windows
            // the locked file is always the one the name refers to.
            #[cfg(unix)]
            if !locks.names_file(name, &file)? {
                continue;
            }
            // Checked under the lock and once the name is known to refer to the locked file,
            // so a foreign file is refused before release could remove it.
            super::claim::ensure_lock_ours(&file)?;
            return Ok(Self {
                name: name.to_os_string(),
                file,
            });
        }
        Err(in_progress())
    }

    /// Ends the lease and removes its lock file from `locks`.
    pub(super) fn release(self, locks: &Anchor) {
        let Self { name, file } = self;
        if cfg!(windows) {
            drop(file);
            let _removed = locks.remove_file(&name);
        } else {
            let _removed = locks.remove_file(&name);
            drop(file);
        }
    }
}

pub(super) fn in_progress() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "another download to this destination is in progress",
    )
}
