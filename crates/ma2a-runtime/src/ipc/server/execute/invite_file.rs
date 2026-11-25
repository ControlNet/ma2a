//! Writing an invitation ticket file readable only by the current account.

use ma2a_core::ProtocolError;

/// Checks an invitation output path before the invitation is created, so a path no write can
/// use is refused before anything is committed: it must name a file that does not exist yet,
/// in an existing directory, by a name this host can create, and may not hold NUL. The name
/// must end the path, as a trailing separator or `.` asks for a directory.
fn check_destination(path: &str) -> Result<(), ProtocolError> {
    let destination = std::path::Path::new(path);
    let name = destination.file_name().and_then(std::ffi::OsStr::to_str);
    let Some(name) = name.filter(|name| !path.contains('\0') && path.ends_with(name)) else {
        return Err(ProtocolError::INVALID_INPUT);
    };
    if cfg!(windows) && (name.contains(':') || ma2a_services::files::is_windows_reserved(name)) {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    if !std::fs::metadata(parent).is_ok_and(|parent| parent.is_dir()) {
        return Err(ProtocolError::INVALID_INPUT);
    }
    // Only a name that is known to be free passes: any other failure, such as a name too
    // long for the file system, would fail the write after the invitation is committed.
    match std::fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(ProtocolError::INVALID_INPUT),
    }
}

/// A fresh name to stage the ticket under, beside its destination.
fn staging_name() -> Result<String, ProtocolError> {
    use std::fmt::Write as _;
    let request = ma2a_core::RequestId::random().map_err(|_| ProtocolError::INTERNAL)?;
    let mut name = String::from(".ma2a-invite-stage-");
    for byte in request.as_bytes() {
        let _written = write!(name, "{byte:02x}");
    }
    Ok(name)
}

/// Where one ticket is going to be written, checked before the invitation is committed.
///
/// On Unix the destination's directory is already open, so a directory the ticket cannot be
/// written through is refused before anything is committed rather than after.
#[cfg(unix)]
pub(super) struct Destination {
    directory: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
}

#[cfg(unix)]
impl Destination {
    /// Checks `path` and opens the directory it names its file in.
    pub(super) fn open(path: &str) -> Result<Self, ProtocolError> {
        check_destination(path)?;
        let (directory, name) = destination_directory(path)?;
        Ok(Self {
            directory,
            name: name.to_owned(),
        })
    }

    /// Writes the ticket into an owner-only staging file in the destination's directory and
    /// links it into place without replacing anything. Until the link, the destination name is
    /// never touched, so a failed or interrupted write leaves it unpublished and cleanup removes
    /// only the staging file, never a file someone else put at the destination.
    ///
    /// Every step runs relative to the directory opened before the commit, so all of them
    /// happen in the same directory: one renamed meanwhile receives the ticket under its new
    /// name, and no staging file is left behind in it.
    pub(super) fn write(self, contents: &[u8]) -> Result<(), ProtocolError> {
        let staged = stage(&self.directory, contents)?;
        publish_staged(&self.directory, &staged, &self.name)
    }
}

/// Opens the directory `path` names its file in, and returns it with that file name.
///
/// Creating, linking, and removing a named file needs only write and search permission on the
/// directory, so where the host can open a directory for search alone it is not also asked to
/// be readable: an owner-only `0300` directory still receives the ticket.
#[cfg(unix)]
fn destination_directory(
    path: &str,
) -> Result<(std::os::fd::OwnedFd, &std::ffi::OsStr), ProtocolError> {
    use rustix::fs::{Mode, OFlags};
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

/// Writes the ticket to a new owner-only staging file in `directory` and returns its name.
#[cfg(unix)]
fn stage(directory: &std::os::fd::OwnedFd, contents: &[u8]) -> Result<String, ProtocolError> {
    use std::io::Write as _;

    use rustix::fs::{AtFlags, Mode, OFlags};
    let name = staging_name()?;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = rustix::fs::openat(directory, name.as_str(), flags, Mode::RUSR | Mode::WUSR)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let mut file = std::fs::File::from(file);
    let written = file
        .write_all(contents)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    if written.is_err() {
        let _removed = rustix::fs::unlinkat(directory, name.as_str(), AtFlags::empty());
        return Err(ProtocolError::INTERNAL);
    }
    Ok(name)
}

/// Gives the staged file the name `name` only if nothing has it, then removes the staging
/// name. A hard link claims the name atomically; a file system without hard links falls back
/// to a no-clobber rename.
#[cfg(unix)]
fn publish_staged(
    directory: &std::os::fd::OwnedFd,
    staged: &str,
    name: &std::ffi::OsStr,
) -> Result<(), ProtocolError> {
    use rustix::{fs::AtFlags, io::Errno};
    let published = match rustix::fs::linkat(directory, staged, directory, name, AtFlags::empty()) {
        Err(error) if error != Errno::EXIST => {
            rename_no_clobber(directory, (staged, name)).map_err(|_| error)
        }
        linked => linked,
    };
    let _removed = rustix::fs::unlinkat(directory, staged, AtFlags::empty());
    published.map_err(|_| ProtocolError::INVALID_INPUT)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_no_clobber(
    directory: &std::os::fd::OwnedFd,
    (from, to): (&str, &std::ffi::OsStr),
) -> rustix::io::Result<()> {
    use rustix::fs::{RenameFlags, renameat_with};
    renameat_with(directory, from, directory, to, RenameFlags::NOREPLACE)
}

/// Other Unix hosts have no no-clobber rename, and a plain one could replace a file.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn rename_no_clobber(
    _directory: &std::os::fd::OwnedFd,
    _names: (&str, &std::ffi::OsStr),
) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::NOSYS)
}

/// Where one ticket is going to be written, checked before the invitation is committed.
#[cfg(windows)]
pub(super) struct Destination(String);

#[cfg(windows)]
impl Destination {
    /// Checks `path`; its directory is pinned when the ticket is written.
    pub(super) fn open(path: &str) -> Result<Self, ProtocolError> {
        check_destination(path)?;
        Ok(Self(path.to_owned()))
    }

    pub(super) fn write(self, contents: &[u8]) -> Result<(), ProtocolError> {
        write_owner_only(&self.0, contents)
    }
}

/// Writes the ticket into a protected staging directory and moves it into place without
/// replacing anything. The destination's directory is pinned first, and staging, publishing,
/// and cleanup all go through its final path, so they all happen in that one directory.
#[cfg(windows)]
fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::{io::Write as _, path::Path};

    use ma2a_services::windows_files;

    check_destination(path)?;
    let destination = Path::new(path);
    let name = destination
        .file_name()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    // A bare file name is relative to the current directory, as it is on Unix.
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let (_pin, directory) =
        windows_files::pin_directory(parent).map_err(|_| ProtocolError::INVALID_INPUT)?;
    let staging = directory.join(staging_name()?);
    windows_files::create_protected_dir(&staging).map_err(|_| ProtocolError::INVALID_INPUT)?;
    let staged = staging.join("ticket");
    let published = (|| {
        let mut file =
            windows_files::create_protected_file(&staged).map_err(|_| ProtocolError::INTERNAL)?;
        file.write_all(contents)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| ProtocolError::INTERNAL)?;
        drop(file);
        windows_files::rename_no_clobber(&staged, &directory.join(name))
            .map_err(|_| ProtocolError::INVALID_INPUT)
    })();
    if published.is_err() {
        let _removed = std::fs::remove_file(&staged);
    }
    let _removed = std::fs::remove_dir(&staging);
    published
}

#[cfg(test)]
#[path = "invite_file_tests.rs"]
mod tests;
