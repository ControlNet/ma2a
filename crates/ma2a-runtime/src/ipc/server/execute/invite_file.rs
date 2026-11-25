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

#[cfg(unix)]
#[path = "invite_file_unix.rs"]
mod unix;
#[cfg(unix)]
pub(super) use unix::Destination;

/// Where one ticket is going to be written, checked before the invitation is committed.
///
/// On Windows the destination's directory is already pinned and the ticket's protected staging
/// directory and file already created in it, so a directory nothing can be created in is refused
/// before anything is committed rather than after. Staging, publishing, and cleanup all go
/// through the pinned directory's final path, so they all happen in that one directory. The
/// staging directory is pinned as well from just after it is made until it is removed, so
/// nothing can move it away and put another directory, with another ticket, at its name. A
/// destination dropped unwritten removes its staging.
#[cfg(windows)]
pub(super) struct Destination {
    _pin: std::fs::File,
    directory: std::path::PathBuf,
    name: std::ffi::OsString,
    staging: std::path::PathBuf,
    staging_pin: Option<std::fs::File>,
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
        // A directory another account swapped in meanwhile is not private, and is left alone.
        let staging_pin = windows_files::pin_private_directory(&staging)
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        let mut opened = Self {
            _pin: pin,
            directory,
            name: name.to_owned(),
            staging,
            staging_pin: Some(staging_pin),
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
        // The pin refuses removal too, so it goes just before the directory does.
        drop(self.staging_pin.take());
        let _removed = std::fs::remove_dir(&self.staging);
    }
}

#[cfg(test)]
#[path = "invite_file_tests.rs"]
mod tests;
