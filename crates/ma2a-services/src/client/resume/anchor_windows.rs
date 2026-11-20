//! A download's directories on Windows, pinned: each is held open without delete sharing, so
//! it can be neither renamed nor removed while the download runs, and every path built from it
//! keeps naming the same directory.

use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// How a helper file is opened.
#[derive(Clone, Copy)]
pub(super) enum Open {
    /// An existing file, for reading only.
    Read,
    /// An existing file, for reading and writing.
    Existing,
    /// A file created when missing, kept as it is otherwise.
    Create,
    /// A file created when missing, emptied otherwise.
    Truncate,
}

/// One pinned directory.
#[derive(Debug)]
pub(super) struct Anchor {
    path: PathBuf,
    _pin: File,
}

fn pin(path: &Path, flags: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | flags)
        .open(path)
}

impl Anchor {
    /// Pins the directory at `path`, following links on the way, as the destination does.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let pinned = pin(path, 0)?;
        if !pinned.metadata()?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self {
            path: path.to_path_buf(),
            _pin: pinned,
        })
    }

    /// Pins the child directory `name`; a reparse point, or anything but a directory, is
    /// refused.
    pub(super) fn child(&self, name: &OsStr) -> io::Result<Self> {
        let path = self.path.join(name);
        let pinned = pin(&path, FILE_FLAG_OPEN_REPARSE_POINT)?;
        let metadata = pinned.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self { path, _pin: pinned })
    }

    /// Creates the child directory `name`; it inherits this directory's ACL, as the
    /// destination does.
    pub(super) fn mkdir_private(&self, name: &OsStr) -> io::Result<()> {
        std::fs::create_dir(self.path.join(name))
    }

    /// Opens the child file `name` without following a reparse point, sharing reads and
    /// writes but never deletion or renaming.
    pub(super) fn open_file(&self, name: &OsStr, open: Open) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match open {
            Open::Read => &mut options,
            Open::Existing => options.write(true),
            Open::Create => options.write(true).create(true).truncate(false),
            Open::Truncate => options.write(true).create(true).truncate(true),
        };
        let file = options.open(self.path.join(name))?;
        let metadata = file.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_file() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        Ok(file)
    }

    /// Returns whether the child `name` is a regular file, and its length, without following
    /// it, or `None` when absent.
    pub(super) fn file_kind(&self, name: &OsStr) -> io::Result<Option<(bool, u64)>> {
        match std::fs::symlink_metadata(self.path.join(name)) {
            Ok(metadata) => Ok(Some((metadata.is_file(), metadata.len()))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        std::fs::remove_file(self.path.join(name))
    }

    pub(super) fn remove_dir(&self, name: &OsStr) -> io::Result<()> {
        std::fs::remove_dir(self.path.join(name))
    }

    /// Renames `from` here to `name` in `to`, replacing an existing file there.
    pub(super) fn rename(&self, from: &OsStr, (to, name): (&Self, &OsStr)) -> io::Result<()> {
        std::fs::rename(self.path.join(from), to.path.join(name))
    }

    /// Renames `from` here to `name` in `to`, failing with `AlreadyExists` instead of
    /// replacing anything.
    pub(super) fn rename_no_clobber(
        &self,
        from: &OsStr,
        (to, name): (&Self, &OsStr),
    ) -> io::Result<()> {
        crate::files::rename_windows::no_clobber_io(&self.path.join(from), &to.path.join(name))
    }
}
