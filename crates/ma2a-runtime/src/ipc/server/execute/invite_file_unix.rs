//! Staging and publishing an invitation ticket on Unix.

use std::{
    ffi::{OsStr, OsString},
    os::fd::OwnedFd,
};

use ma2a_core::ProtocolError;
use rustix::fs::{AtFlags, Mode, OFlags};

use super::{check_destination, staging_name};

/// The ticket's name inside its staging directory.
const TICKET: &str = "ticket";

/// Where one ticket is going to be written, checked before the invitation is committed.
///
/// The destination's directory is already open and the ticket's staging directory and file
/// already created in it, so a directory no file can be created in is refused before anything
/// is committed rather than after. A destination dropped unwritten removes its staging.
pub(in super::super) struct Destination {
    directory: OwnedFd,
    name: OsString,
    staged: Option<Staged>,
}

/// An owner-only staging directory beside the destination, held open, and the ticket file
/// open inside it. Every later step goes through these handles, never through the staging
/// name, so whatever another process puts at that name is never written, published, or
/// removed in the ticket's place.
pub(super) struct Staged {
    name: String,
    pub(super) directory: OwnedFd,
    file: std::fs::File,
}

impl Destination {
    /// Checks `path`, opens the directory it names its file in, and stages the file there.
    pub(in super::super) fn open(path: &str) -> Result<Self, ProtocolError> {
        check_destination(path)?;
        let (directory, name) = destination_directory(path)?;
        let staged = create_staging(&directory)?;
        Ok(Self {
            directory,
            name: name.to_owned(),
            staged: Some(staged),
        })
    }

    /// Writes the ticket into the staging file and links it into place without replacing
    /// anything. Until the link, the destination name is never touched, so a failed or
    /// interrupted write leaves it unpublished and cleanup removes only the staging, never a
    /// file someone else put at the destination.
    ///
    /// Every step runs relative to the directories opened before the commit, so all of them
    /// happen in the same directory: one renamed meanwhile receives the ticket under its new
    /// name, and no staging is left behind in it.
    pub(in super::super) fn write(mut self, contents: &[u8]) -> Result<(), ProtocolError> {
        let mut staged = self.staged.take().ok_or(ProtocolError::INTERNAL)?;
        let published = fill(&mut staged, contents)
            .and_then(|()| publish_staged(&self.directory, &staged, &self.name));
        remove_staging(&self.directory, &staged);
        published
    }
}

impl Drop for Destination {
    fn drop(&mut self) {
        if let Some(staged) = self.staged.take() {
            remove_staging(&self.directory, &staged);
        }
    }
}

/// Opens the directory `path` names its file in, and returns it with that file name.
///
/// Creating, linking, and removing a named entry needs only write and search permission on the
/// directory, so where the host can open a directory for search alone it is not also asked to
/// be readable: an owner-only `0300` directory still receives the ticket.
pub(super) fn destination_directory(path: &str) -> Result<(OwnedFd, &OsStr), ProtocolError> {
    let destination = std::path::Path::new(path);
    let name = destination
        .file_name()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let access = OFlags::PATH;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let access = OFlags::RDONLY;
    let flags = access | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let directory =
        rustix::fs::open(parent, flags, Mode::empty()).map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok((directory, name))
}

/// Creates an owner-only staging directory in `directory` and the ticket file in it.
///
/// The directory is opened by name once after it is made, so it is checked to be one only this
/// account can change: a directory another account swapped in between is refused.
pub(super) fn create_staging(directory: &OwnedFd) -> Result<Staged, ProtocolError> {
    let name = staging_name()?;
    rustix::fs::mkdirat(directory, name.as_str(), Mode::RWXU)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let opened = open_staging(directory, &name);
    if opened.is_err() {
        let _removed = rustix::fs::unlinkat(directory, name.as_str(), AtFlags::REMOVEDIR);
    }
    let (staging, file) = opened?;
    Ok(Staged {
        name,
        directory: staging,
        file: std::fs::File::from(file),
    })
}

/// Opens the staging directory just made at `name`, refusing one this account does not own
/// alone, and creates the ticket file in it.
fn open_staging(directory: &OwnedFd, name: &str) -> Result<(OwnedFd, OwnedFd), ProtocolError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let staging = rustix::fs::openat(directory, name, flags, Mode::empty())
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let stat = rustix::fs::fstat(&staging).map_err(|_| ProtocolError::INTERNAL)?;
    let shared = Mode::from_bits_truncate(stat.st_mode).intersects(Mode::RWXG | Mode::RWXO);
    if stat.st_uid != rustix::process::geteuid().as_raw() || shared {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = rustix::fs::openat(&staging, TICKET, flags, Mode::RUSR | Mode::WUSR)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok((staging, file))
}

/// Writes the ticket through the staging file's own handle.
pub(super) fn fill(staged: &mut Staged, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::io::Write as _;
    staged
        .file
        .write_all(contents)
        .and_then(|()| staged.file.write_all(b"\n"))
        .and_then(|()| staged.file.sync_all())
        .map_err(|_| ProtocolError::INTERNAL)
}

/// Gives the staged ticket the name `name` in `directory` only if nothing has it. A hard link
/// claims the name atomically; a file system without hard links falls back to a no-clobber
/// rename.
pub(super) fn publish_staged(
    directory: &OwnedFd,
    staged: &Staged,
    name: &OsStr,
) -> Result<(), ProtocolError> {
    use rustix::io::Errno;
    let staging = &staged.directory;
    match rustix::fs::linkat(staging, TICKET, directory, name, AtFlags::empty()) {
        Err(error) if error != Errno::EXIST => {
            rename_no_clobber((staging, directory), name).map_err(|_| error)
        }
        linked => linked,
    }
    .map_err(|_| ProtocolError::INVALID_INPUT)
}

/// Removes the ticket from the staging directory, then the staging directory if it is still
/// at its name and empty: removing a directory never removes anything in it, and whatever else
/// took the name, such as an empty directory another process made there after moving the
/// staging away, is left alone. Nothing removes a directory only if it is a given one, so the
/// check before the removal narrows that race to the two calls but cannot close it.
fn remove_staging(directory: &OwnedFd, staged: &Staged) {
    let _removed = rustix::fs::unlinkat(&staged.directory, TICKET, AtFlags::empty());
    let held = rustix::fs::fstat(&staged.directory);
    let at_name = rustix::fs::statat(directory, staged.name.as_str(), AtFlags::SYMLINK_NOFOLLOW);
    if let (Ok(held), Ok(at_name)) = (held, at_name)
        && (held.st_dev, held.st_ino) == (at_name.st_dev, at_name.st_ino)
    {
        let _removed = rustix::fs::unlinkat(directory, staged.name.as_str(), AtFlags::REMOVEDIR);
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(super) fn rename_no_clobber(
    (staging, directory): (&OwnedFd, &OwnedFd),
    name: &OsStr,
) -> rustix::io::Result<()> {
    use rustix::fs::{RenameFlags, renameat_with};
    renameat_with(staging, TICKET, directory, name, RenameFlags::NOREPLACE)
}

/// Other Unix hosts have no no-clobber rename, and a plain one could replace a file.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub(super) fn rename_no_clobber(
    _directories: (&OwnedFd, &OwnedFd),
    _name: &OsStr,
) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::NOSYS)
}
