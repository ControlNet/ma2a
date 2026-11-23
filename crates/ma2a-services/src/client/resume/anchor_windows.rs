//! A download's directories on Windows, pinned: each is held open without delete sharing, so
//! it can be neither renamed nor removed while the download runs, and neither can a directory
//! above it, since Windows refuses to rename a directory with open handles beneath it. Each is
//! reached afterwards by the path its handle resolves to, which has no link left to repoint,
//! and files are recognized by their identity on the volume, as on Unix.

use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
};

use crate::windows_files::{self, FileId};

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
    /// A file that must not exist yet.
    New,
}

/// One pinned directory.
#[derive(Debug)]
pub(super) struct Anchor {
    path: PathBuf,
    pin: File,
}

fn pin(path: &Path, flags: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | flags)
        .open(path)
}

/// Returns the identity of `name` in `directory` without following it, or `None` when absent.
fn identity_at(directory: &Path, name: &OsStr) -> io::Result<Option<FileId>> {
    let opened = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.join(name));
    match opened {
        Ok(file) => Ok(Some(windows_files::file_id(&file)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Which file a held handle refers to.
pub(super) fn id_of(file: &File) -> io::Result<FileId> {
    windows_files::file_id(file)
}

impl Anchor {
    /// Pins the directory at `path`, following links on the way, as the destination does.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let pin = pin(path, 0)?;
        if !pin.metadata()?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        let path = windows_files::final_path(&pin)?;
        Ok(Self { path, pin })
    }

    /// Pins the child directory `name`; a reparse point, or anything but a directory, is
    /// refused.
    pub(super) fn child(&self, name: &OsStr) -> io::Result<Self> {
        let path = self.path.join(name);
        let pin = pin(&path, FILE_FLAG_OPEN_REPARSE_POINT)?;
        let metadata = pin.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self { path, pin })
    }

    /// Creates the child directory `name` under a protected ACL for this account and SYSTEM,
    /// which everything made inside inherits.
    pub(super) fn mkdir_private(&self, name: &OsStr) -> io::Result<()> {
        windows_files::create_protected_dir(&self.path.join(name))
    }

    /// Whether this account, SYSTEM, or the Administrators group owns this directory and its
    /// protected ACL lets no other account in.
    pub(super) fn is_private(&self) -> io::Result<bool> {
        let user = crate::windows_security::current_process_user_sid()?.to_sddl()?;
        let sddl = windows_files::owner_and_dacl(&self.pin)?;
        Ok(crate::sddl::is_private(&sddl, &user))
    }

    /// Opens the child file `name` without following a reparse point, never sharing deletion
    /// or renaming. Only a lock file shares writing, so every download can open it to try its
    /// lock; any other helper file, once open, cannot be written through another handle.
    pub(super) fn open_file(&self, name: &OsStr, open: Open) -> io::Result<File> {
        let share = match open {
            Open::Create => FILE_SHARE_READ | FILE_SHARE_WRITE,
            Open::Read | Open::Existing | Open::Truncate | Open::New => FILE_SHARE_READ,
        };
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(share)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match open {
            Open::Read => &mut options,
            Open::Existing => options.write(true),
            Open::Create => options.write(true).create(true).truncate(false),
            Open::Truncate => options.write(true).create(true).truncate(true),
            Open::New => options.write(true).create_new(true),
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

    /// Whether the child `name` is, without following it, the file `held`.
    pub(super) fn names_file(&self, name: &OsStr, held: &File) -> io::Result<bool> {
        self.names(name, id_of(held)?)
    }

    /// Whether the child `name` is, without following it, the directory `held`.
    pub(super) fn names_directory(&self, name: &OsStr, held: &Self) -> io::Result<bool> {
        self.names(name, id_of(&held.pin)?)
    }

    /// Whether the child `name` is, without following it, the file identified by `held`.
    pub(super) fn names(&self, name: &OsStr, held: FileId) -> io::Result<bool> {
        Ok(identity_at(&self.path, name)? == Some(held))
    }

    /// Whether `path` still names this directory.
    pub(super) fn is_at(&self, path: &Path) -> io::Result<bool> {
        let opened = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path);
        match opened {
            Ok(opened) => Ok(id_of(&opened)? == id_of(&self.pin)?),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
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
