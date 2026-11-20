//! The directories one download works in, opened once when it starts.
//!
//! The destination's directory, the private directory beside it, and its lock directory are
//! held for the whole download, and every helper file is reached through them. A rename of the
//! destination's directory afterwards therefore never redirects the download into another
//! directory that carries the same names; on Unix publishing then refuses, keeping the
//! progress, and on Windows the held directories cannot be renamed at all.

use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use super::{
    anchor::{Anchor, Open},
    lease::{Lease, in_progress},
    place::{CREATED, LOCKS, PRIVATE_DIRECTORY, foreign_directory},
};

/// How often a private directory another download removed meanwhile is made again.
const ATTEMPTS: usize = 4;

/// The held directories of one download.
#[derive(Debug)]
pub(super) struct Helpers {
    /// The destination's directory, and the path it was opened by.
    pub(super) parent: (Anchor, PathBuf),
    /// The private directory beside the destination.
    pub(super) directory: Anchor,
    /// The lock directory inside it.
    pub(super) locks: Anchor,
}

impl Helpers {
    /// Opens the directories of a download to `name` in `parent`, making the private
    /// directory when missing, and takes the destination's lease.
    ///
    /// An existing private directory is used only when, on Unix, this account owns it and no
    /// other account may enter it; anything else carrying its name is refused and kept.
    pub(super) fn open(parent: &Path, name: &OsStr) -> io::Result<(Self, Lease)> {
        let private = OsStr::new(PRIVATE_DIRECTORY);
        for _ in 0..ATTEMPTS {
            let anchor = Anchor::open(parent)?;
            let created = match anchor.mkdir_private(private) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error),
            };
            let directory = match anchor.child(private) {
                Ok(directory) => directory,
                // Removed by a download that ended meanwhile; make it again.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    return Err(error);
                }
                Err(_) => return Err(foreign_directory()),
            };
            #[cfg(unix)]
            if !directory.is_private()? {
                return Err(foreign_directory());
            }
            let Some(opened) = Self::inside((anchor, parent), directory, (created, name))? else {
                continue;
            };
            return Ok(opened);
        }
        Err(in_progress())
    }

    /// Finishes opening inside the private `directory`; `None` when another download removed
    /// it meanwhile.
    fn inside(
        (parent, path): (Anchor, &Path),
        directory: Anchor,
        (created, name): (bool, &OsStr),
    ) -> io::Result<Option<(Self, Lease)>> {
        let gone = |error: &io::Error| error.kind() == io::ErrorKind::NotFound;
        if created {
            match directory.open_file(OsStr::new(CREATED), Open::Create) {
                Err(error) if gone(&error) => return Ok(None),
                opened => drop(opened?),
            }
        }
        match directory.mkdir_private(OsStr::new(LOCKS)) {
            Err(error) if gone(&error) => return Ok(None),
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        let locks = match directory.child(OsStr::new(LOCKS)) {
            Err(error) if gone(&error) => return Ok(None),
            locks => locks?,
        };
        let lease = match Lease::acquire(&locks, name) {
            Err(error) if gone(&error) => return Ok(None),
            lease => lease?,
        };
        let helpers = Self {
            parent: (parent, path.to_path_buf()),
            directory,
            locks,
        };
        // The private directory, or its lock directory, may have been removed and made again
        // by other downloads between opening and locking; then this lease guards nothing.
        #[cfg(unix)]
        if !helpers.still_named()? {
            lease.release(&helpers.locks);
            return Ok(None);
        }
        Ok(Some((helpers, lease)))
    }

    #[cfg(unix)]
    fn still_named(&self) -> io::Result<bool> {
        Ok(self
            .parent
            .0
            .names_directory(OsStr::new(PRIVATE_DIRECTORY), &self.directory)?
            && self
                .directory
                .names_directory(OsStr::new(LOCKS), &self.locks)?)
    }

    /// Removes the lock directory once no lease is left in it, and the private directory once
    /// it is empty, but only one a download made: an operator's own directory is kept.
    pub(super) fn tidy(self) {
        let Self {
            parent: (parent, _),
            directory,
            locks,
        } = self;
        drop(locks);
        let _removed = directory.remove_dir(OsStr::new(LOCKS));
        let created = OsStr::new(CREATED);
        if !matches!(directory.file_kind(created), Ok(Some((true, _)))) {
            return;
        }
        #[cfg(unix)]
        if !parent
            .names_directory(OsStr::new(PRIVATE_DIRECTORY), &directory)
            .unwrap_or(false)
        {
            return;
        }
        if directory.remove_file(created).is_err() {
            return;
        }
        // The held directory cannot be removed on Windows while it is pinned.
        drop(directory);
        if parent.remove_dir(OsStr::new(PRIVATE_DIRECTORY)).is_err() {
            // Another download still keeps files there; its end tidies instead.
            if let Ok(directory) = parent.child(OsStr::new(PRIVATE_DIRECTORY)) {
                let _marked = directory.open_file(created, Open::Create);
            }
        }
    }
}
