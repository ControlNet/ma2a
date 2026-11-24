//! Directory handles on Unix: every step is an `*at` call relative to a verified parent.
//!
//! A path is never walked by name after it was checked. Each component is opened with
//! `O_DIRECTORY | O_NOFOLLOW` from the handle of its parent, so renaming or replacing an
//! ancestor with a symbolic link while an operation runs cannot redirect it outside the root.

use std::{
    ffi::OsStr,
    fs::File,
    io,
    ops::ControlFlow,
    os::{fd::OwnedFd, unix::ffi::OsStrExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use ma2a_core::service::{FileKind, ServiceError};
use rustix::{
    fs::{self as at, AtFlags, FileType, Mode, OFlags},
    io::Errno,
};

use super::{entry::Entry, stop::Stop};
use crate::{client::resume::is_private_directory, fs_error};

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// One open directory inside an exported root, and its logical path for journaling.
#[derive(Clone, Debug)]
pub(crate) struct Directory {
    fd: Arc<OwnedFd>,
    path: PathBuf,
}

fn io(errno: Errno) -> io::Error {
    io::Error::from(errno)
}

impl Directory {
    /// Opens the canonical root directory recorded at exposure time.
    pub(crate) fn open_root(path: &Path) -> Result<Self, ServiceError> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = fs_error::map(at::openat(at::CWD, path, flags, Mode::empty()).map_err(io))?;
        Ok(Self {
            fd: Arc::new(fd),
            path: path.to_path_buf(),
        })
    }

    /// Opens a child directory without following a symbolic link.
    pub(crate) fn child(&self, name: &str) -> Result<Self, ServiceError> {
        match at::openat(&*self.fd, name, DIRECTORY, Mode::empty()) {
            Ok(fd) => Ok(Self {
                fd: Arc::new(fd),
                path: self.path.join(name),
            }),
            // Platforms disagree on the errno for a final link under `O_NOFOLLOW`, so the
            // entry itself decides which refusal applies.
            Err(errno) => match self.entry(name)? {
                Some(entry) if entry.kind == FileKind::Symlink => Err(ServiceError::SymlinkRefused),
                Some(entry) if entry.kind != FileKind::Directory => {
                    Err(ServiceError::NotADirectory)
                }
                _ => Err(fs_error::service_error(&io(errno))),
            },
        }
    }

    /// Returns the logical path of this directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Returns metadata of the directory itself.
    pub(crate) fn itself(&self) -> Result<Entry, ServiceError> {
        Ok(entry(&fs_error::map(at::fstat(&*self.fd).map_err(io))?))
    }

    /// Returns metadata of one child without following it, or `None` when absent.
    pub(crate) fn entry(&self, name: &str) -> Result<Option<Entry>, ServiceError> {
        match at::statat(&*self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(entry(&stat))),
            Err(Errno::NOENT) => Ok(None),
            Err(errno) => Err(fs_error::service_error(&io(errno))),
        }
    }

    /// Visits every child name; names that are not UTF-8 are passed as `None`.
    /// Visits every entry name until `visit` breaks.
    pub(crate) fn for_each_name(
        &self,
        mut visit: impl FnMut(Option<&str>) -> ControlFlow<()>,
    ) -> io::Result<()> {
        for child in at::Dir::read_from(&*self.fd).map_err(io)? {
            let child = child.map_err(io)?;
            let name = child.file_name().to_bytes();
            if name != b"." && name != b".." && visit(OsStr::from_bytes(name).to_str()).is_break() {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn mkdir(&self, name: &str) -> io::Result<()> {
        at::mkdirat(&*self.fd, name, Mode::RWXU | Mode::RWXG | Mode::RWXO).map_err(io)
    }

    pub(crate) fn remove_file(&self, name: &str) -> io::Result<()> {
        at::unlinkat(&*self.fd, name, AtFlags::empty()).map_err(io)
    }

    /// Removes a non-directory entry.
    pub(crate) fn remove_entry(&self, name: &str) -> io::Result<()> {
        self.remove_file(name)
    }

    pub(crate) fn remove_dir(&self, name: &str) -> io::Result<()> {
        at::unlinkat(&*self.fd, name, AtFlags::REMOVEDIR).map_err(io)
    }

    /// Removes a directory tree; links inside it are removed, never followed.
    ///
    /// A download's private directory inside the tree is kept with everything in it, so its
    /// progress survives; the directories above it then stay too, and the removal ends with
    /// `not_empty` after removing everything else.
    ///
    /// Only `depth` levels of directories below `name` are entered; anything deeper is kept, as
    /// a kept download is, so the work and handles one removal holds stay bounded.
    pub(crate) fn remove_tree(
        &self,
        name: &str,
        (stop, depth): (&Stop, usize),
    ) -> Result<(), ServiceError> {
        let directory = self.child(name)?;
        // Entries are removed while the directory is read, so memory stays bounded.
        for child in fs_error::map(at::Dir::read_from(&*directory.fd).map_err(io))? {
            if stop.is_set() {
                return Err(ServiceError::Cancelled);
            }
            let child = fs_error::map(child.map_err(io))?;
            let bytes = child.file_name().to_bytes();
            // A non-UTF-8 name cannot be addressed remotely; it keeps the directory non-empty.
            let Some(child) = OsStr::from_bytes(bytes).to_str() else {
                continue;
            };
            if child == "." || child == ".." || is_private_directory(OsStr::new(child)) {
                continue;
            }
            match directory.entry(child)? {
                // A subdirectory left non-empty by a kept download does not stop its siblings.
                Some(entry) if entry.kind == FileKind::Directory => {
                    let Some(below) = depth.checked_sub(1) else {
                        continue;
                    };
                    match directory.remove_tree(child, (stop, below)) {
                        Err(ServiceError::NotEmpty) => {}
                        removed => removed?,
                    }
                }
                Some(_) => fs_error::map(directory.remove_file(child))?,
                None => {}
            }
        }
        if stop.is_set() {
            return Err(ServiceError::Cancelled);
        }
        fs_error::map(self.remove_dir(name))
    }

    /// Renames `from` in this directory to `to`, replacing an existing destination.
    pub(crate) fn rename(&self, from: &str, (to, name): (&Self, &str)) -> io::Result<()> {
        at::renameat(&*self.fd, from, &*to.fd, name).map_err(io)
    }

    /// Renames `from` to `to`, failing with `already_exists` instead of replacing it.
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    pub(crate) fn rename_no_clobber(
        &self,
        from: &str,
        (to, name): (&Self, &str),
    ) -> Result<(), ServiceError> {
        let flags = at::RenameFlags::NOREPLACE;
        fs_error::map(at::renameat_with(&*self.fd, from, &*to.fd, name, flags).map_err(io))
    }

    /// Other Unix hosts lack a no-clobber rename; a hard link reserves the name atomically
    /// for files, and directories are refused rather than raced.
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    pub(crate) fn rename_no_clobber(
        &self,
        from: &str,
        (to, name): (&Self, &str),
    ) -> Result<(), ServiceError> {
        if self
            .entry(from)?
            .is_some_and(|entry| entry.kind == FileKind::Directory)
        {
            return Err(ServiceError::Io);
        }
        fs_error::map(at::linkat(&*self.fd, from, &*to.fd, name, AtFlags::empty()).map_err(io))?;
        fs_error::map(self.remove_file(from))
    }

    /// Links `from` under the new name `to` in this directory.
    pub(crate) fn hard_link(&self, from: &str, to: &str) -> io::Result<()> {
        at::linkat(&*self.fd, from, &*self.fd, to, AtFlags::empty()).map_err(io)
    }

    /// Opens an existing child for reading without following it or blocking on a FIFO.
    pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = at::openat(&*self.fd, name, flags, Mode::empty()).map_err(io)?;
        let status = at::fcntl_getfl(&fd).map_err(io)?;
        at::fcntl_setfl(&fd, status.difference(OFlags::NONBLOCK)).map_err(io)?;
        Ok(File::from(fd))
    }

    /// Creates a child file exclusively; an existing entry or link is never reused.
    pub(crate) fn create_new(&self, name: &str) -> io::Result<File> {
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW;
        let mode = Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::WGRP | Mode::ROTH | Mode::WOTH;
        let fd = at::openat(&*self.fd, name, flags | OFlags::CLOEXEC, mode).map_err(io)?;
        Ok(File::from(fd))
    }

    /// Makes new directory entries durable.
    pub(crate) fn sync(&self) {
        let _synced = at::fsync(&*self.fd);
    }
}

/// Converts `stat` fields whose widths differ between Unix targets.
fn entry(stat: &at::Stat) -> Entry {
    let kind = match FileType::from_raw_mode(stat.st_mode) {
        FileType::Directory => FileKind::Directory,
        FileType::RegularFile => FileKind::File,
        FileType::Symlink => FileKind::Symlink,
        _ => FileKind::Other,
    };
    let seconds = i128::from(stat.st_mtime);
    let nanoseconds = i128::from(stat.st_mtime_nsec);
    let modified_ms = u64::try_from(seconds * 1_000 + nanoseconds / 1_000_000).ok();
    let writable = Mode::WUSR | Mode::WGRP | Mode::WOTH;
    Entry {
        kind,
        len: u64::try_from(i128::from(stat.st_size)).unwrap_or(0),
        modified_ms,
        read_only: !Mode::from_raw_mode(stat.st_mode).intersects(writable),
    }
}
