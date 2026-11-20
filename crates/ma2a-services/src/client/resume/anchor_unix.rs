//! A download's directories on Unix, held open: every helper-file operation is an `*at` call
//! relative to a directory opened once, so renaming a directory on the path afterwards cannot
//! redirect it into another directory that happens to carry the same names.

use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::{fd::OwnedFd, unix::fs::MetadataExt as _},
    path::Path,
};

use rustix::fs::{self as at, AtFlags, Mode, OFlags, Stat};

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

/// One open directory.
#[derive(Debug)]
pub(super) struct Anchor {
    fd: OwnedFd,
}

impl Anchor {
    /// Opens the directory at `path`, following links on the way, as the destination does.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        Ok(Self {
            fd: at::openat(at::CWD, path, flags, Mode::empty())?,
        })
    }

    /// Opens the child directory `name`; a link, or anything but a directory, is refused.
    pub(super) fn child(&self, name: &OsStr) -> io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        Ok(Self {
            fd: at::openat(&self.fd, name, flags, Mode::empty())?,
        })
    }

    /// Creates the child directory `name`, which only this account may enter.
    pub(super) fn mkdir_private(&self, name: &OsStr) -> io::Result<()> {
        Ok(at::mkdirat(&self.fd, name, Mode::RWXU)?)
    }

    /// Whether this account owns this directory and no other account may list or enter it.
    pub(super) fn is_private(&self) -> io::Result<bool> {
        let stat = at::fstat(&self.fd)?;
        let shared = stat.st_mode & 0o077 != 0;
        Ok(stat.st_uid == rustix::process::geteuid().as_raw() && !shared)
    }

    /// Opens the child file `name` without following a link; a new file is owner-only.
    pub(super) fn open_file(&self, name: &OsStr, open: Open) -> io::Result<File> {
        let base = OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let flags = match open {
            Open::Read => OFlags::RDONLY | OFlags::NONBLOCK,
            Open::Existing => OFlags::RDWR | OFlags::NONBLOCK,
            Open::Create => OFlags::RDWR | OFlags::CREATE | OFlags::NONBLOCK,
            Open::Truncate => OFlags::RDWR | OFlags::CREATE | OFlags::TRUNC | OFlags::NONBLOCK,
        };
        let fd = at::openat(&self.fd, name, base | flags, Mode::RUSR | Mode::WUSR)?;
        // Opened without blocking so a FIFO cannot stall the download; refused below.
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let status = at::fcntl_getfl(&file)?;
        at::fcntl_setfl(&file, status.difference(OFlags::NONBLOCK))?;
        Ok(file)
    }

    /// Returns whether the child `name` is a regular file, and its length, without following
    /// it, or `None` when absent.
    pub(super) fn file_kind(&self, name: &OsStr) -> io::Result<Option<(bool, u64)>> {
        match at::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                let regular = at::FileType::from_raw_mode(stat.st_mode).is_file();
                Ok(Some((regular, u64::try_from(stat.st_size).unwrap_or(0))))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    /// Whether the child `name` is, without following it, the file `held`.
    pub(super) fn names_file(&self, name: &OsStr, held: &File) -> io::Result<bool> {
        let held = held.metadata()?;
        self.names(name, (held.dev(), held.ino()))
    }

    /// Whether the child `name` is, without following it, the directory `held`.
    pub(super) fn names_directory(&self, name: &OsStr, held: &Self) -> io::Result<bool> {
        let held = at::fstat(&held.fd)?;
        self.names(name, identity(&held))
    }

    /// Whether `path` still names this directory.
    pub(super) fn is_at(&self, path: &Path) -> io::Result<bool> {
        let named = std::fs::metadata(path)?;
        Ok(identity(&at::fstat(&self.fd)?) == (named.dev(), named.ino()))
    }

    fn names(&self, name: &OsStr, held: (u64, u64)) -> io::Result<bool> {
        match at::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(named) => Ok(identity(&named) == held),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(errno) => Err(errno.into()),
        }
    }

    pub(super) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        Ok(at::unlinkat(&self.fd, name, AtFlags::empty())?)
    }

    pub(super) fn remove_dir(&self, name: &OsStr) -> io::Result<()> {
        Ok(at::unlinkat(&self.fd, name, AtFlags::REMOVEDIR)?)
    }

    /// Renames `from` here to `name` in `to`, replacing an existing file there.
    pub(super) fn rename(&self, from: &OsStr, (to, name): (&Self, &OsStr)) -> io::Result<()> {
        Ok(at::renameat(&self.fd, from, &to.fd, name)?)
    }

    /// Renames `from` here to `name` in `to`, failing with `AlreadyExists` instead of
    /// replacing anything. Where the file system has no such rename, a hard link reserves the
    /// name instead; without either, the publish fails rather than risk a replacement.
    pub(super) fn rename_no_clobber(
        &self,
        from: &OsStr,
        (to, name): (&Self, &OsStr),
    ) -> io::Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        match at::renameat_with(&self.fd, from, &to.fd, name, at::RenameFlags::NOREPLACE) {
            Err(
                rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP,
            ) => {}
            renamed => return Ok(renamed?),
        }
        at::linkat(&self.fd, from, &to.fd, name, AtFlags::empty())?;
        self.remove_file(from)
    }
}

/// The device and inode `stat` describes, widened to fixed types.
fn identity(stat: &Stat) -> (u64, u64) {
    #[expect(
        clippy::useless_conversion,
        reason = "the field widths differ between Unix targets"
    )]
    (u64::from(stat.st_dev), u64::from(stat.st_ino))
}
