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
/// On Unix the destination's directory is already open and the ticket's staging file already
/// created in it, so a directory no file can be created in is refused before anything is
/// committed rather than after. A destination dropped unwritten removes its staging file.
#[cfg(unix)]
pub(super) struct Destination {
    directory: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
    staged: Option<Staged>,
}

/// An owner-only staging file and its name in the destination's directory.
#[cfg(unix)]
type Staged = (String, std::fs::File);

#[cfg(unix)]
impl Destination {
    /// Checks `path`, opens the directory it names its file in, and stages the file there.
    pub(super) fn open(path: &str) -> Result<Self, ProtocolError> {
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
    /// interrupted write leaves it unpublished and cleanup removes only the staging file, never
    /// a file someone else put at the destination.
    ///
    /// Every step runs relative to the directory opened before the commit, so all of them
    /// happen in the same directory: one renamed meanwhile receives the ticket under its new
    /// name, and no staging file is left behind in it.
    pub(super) fn write(mut self, contents: &[u8]) -> Result<(), ProtocolError> {
        let staged = self.staged.take().ok_or(ProtocolError::INTERNAL)?;
        let staged = fill(&self.directory, staged, contents)?;
        publish_staged(&self.directory, &staged, &self.name)
    }
}

#[cfg(unix)]
impl Drop for Destination {
    fn drop(&mut self) {
        if let Some((name, _file)) = self.staged.take() {
            let _removed =
                rustix::fs::unlinkat(&self.directory, name.as_str(), rustix::fs::AtFlags::empty());
        }
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

/// Creates a new owner-only staging file in `directory`.
#[cfg(unix)]
fn create_staging(directory: &std::os::fd::OwnedFd) -> Result<Staged, ProtocolError> {
    use rustix::fs::{Mode, OFlags};
    let name = staging_name()?;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = rustix::fs::openat(directory, name.as_str(), flags, Mode::RUSR | Mode::WUSR)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok((name, std::fs::File::from(file)))
}

/// Writes the ticket to its staging file and returns the file's name; a failed write removes
/// the file.
#[cfg(unix)]
fn fill(
    directory: &std::os::fd::OwnedFd,
    (name, mut file): Staged,
    contents: &[u8],
) -> Result<String, ProtocolError> {
    use std::io::Write as _;

    use rustix::fs::AtFlags;
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
///
/// On Windows the destination's directory is already pinned and the ticket's protected staging
/// directory and file already created in it, so a directory nothing can be created in is refused
/// before anything is committed rather than after. Staging, publishing, and cleanup all go
/// through the pinned directory's final path, so they all happen in that one directory. A
/// destination dropped unwritten removes its staging.
#[cfg(windows)]
pub(super) struct Destination {
    _pin: std::fs::File,
    directory: std::path::PathBuf,
    name: std::ffi::OsString,
    staging: std::path::PathBuf,
    file: Option<std::fs::File>,
}

#[cfg(windows)]
impl Destination {
    /// Checks `path`, pins the directory it names its file in, and stages the file there.
    pub(super) fn open(path: &str) -> Result<Self, ProtocolError> {
        use std::path::Path;

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
        let (pin, directory) =
            windows_files::pin_directory(parent).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let staging = directory.join(staging_name()?);
        windows_files::create_protected_dir(&staging).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let mut opened = Self {
            _pin: pin,
            directory,
            name: name.to_owned(),
            staging,
            file: None,
        };
        let file = windows_files::create_protected_file(&opened.staged())
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        opened.file = Some(file);
        Ok(opened)
    }

    /// Writes the ticket into the staging file and moves it into place without replacing
    /// anything; dropping the destination then removes whatever staging is left.
    pub(super) fn write(mut self, contents: &[u8]) -> Result<(), ProtocolError> {
        use std::io::Write as _;
        let mut file = self.file.take().ok_or(ProtocolError::INTERNAL)?;
        file.write_all(contents)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| ProtocolError::INTERNAL)?;
        // The file is open without sharing, so it is closed before it can be moved.
        drop(file);
        ma2a_services::windows_files::rename_no_clobber(
            &self.staged(),
            &self.directory.join(&self.name),
        )
        .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    fn staged(&self) -> std::path::PathBuf {
        self.staging.join("ticket")
    }
}

#[cfg(windows)]
impl Drop for Destination {
    fn drop(&mut self) {
        drop(self.file.take());
        let _removed = std::fs::remove_file(self.staged());
        let _removed = std::fs::remove_dir(&self.staging);
    }
}

#[cfg(test)]
#[path = "invite_file_tests.rs"]
mod tests;
