//! Writing an invitation ticket file readable only by the current account.

use ma2a_core::ProtocolError;

/// Checks an invitation output path before the invitation is created, so a path no write can
/// use is refused before anything is committed: it must name a file that does not exist yet,
/// in an existing directory, by a name this host can create, and may not hold NUL. The name
/// must end the path, as a trailing separator or `.` asks for a directory.
pub(super) fn check_destination(path: &str) -> Result<(), ProtocolError> {
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

/// Writes the ticket into an owner-only staging file in the destination's directory and links
/// it into place without replacing anything. Until the link, the destination name is never
/// touched, so a failed or interrupted write leaves it unpublished and cleanup removes only the
/// staging file, never a file someone else put at the destination.
#[cfg(unix)]
pub(super) fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _, path::Path};
    check_destination(path)?;
    let destination = Path::new(path);
    let staged = destination.with_file_name(staging_name()?);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let published = file
        .write_all(contents)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| ProtocolError::INTERNAL)
        .and_then(|()| publish(&staged, destination).map_err(|_| ProtocolError::INVALID_INPUT));
    drop(file);
    let _removed = std::fs::remove_file(&staged);
    published
}

/// Gives `staged` the name `destination` only if nothing has it. A hard link claims the name
/// atomically; a file system without hard links falls back to a no-clobber rename.
#[cfg(unix)]
fn publish(staged: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    match std::fs::hard_link(staged, destination) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            rename_no_clobber(staged, destination).map_err(|_| error)
        }
        linked => linked,
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_no_clobber(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from)
}

/// Other Unix hosts have no no-clobber rename, and a plain one could replace a file.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn rename_no_clobber(_from: &std::path::Path, _to: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Writes the ticket into a protected staging directory and moves it into place without
/// replacing anything. The destination's directory is pinned first, and staging, publishing,
/// and cleanup all go through its final path, so they all happen in that one directory.
#[cfg(windows)]
pub(super) fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
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
mod tests {
    use super::check_destination;

    #[test]
    fn only_a_new_file_in_an_existing_directory_is_accepted() -> Result<(), std::io::Error> {
        let directory =
            std::env::temp_dir().join(format!("ma2a-invite-check-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join("taken"), b"")?;
        let path = |name: &str| directory.join(name).to_string_lossy().into_owned();

        assert!(check_destination(&path("ticket")).is_ok());
        assert!(check_destination(&path("taken")).is_err());
        assert!(check_destination(&path("missing/ticket")).is_err());
        assert!(check_destination(&path("taken/ticket")).is_err());
        assert!(check_destination("").is_err());
        assert!(check_destination(&path("..")).is_err());
        assert!(check_destination(&path("ti\0cket")).is_err());
        assert!(check_destination(&format!("{}/", path("ticket"))).is_err());
        assert!(check_destination(&format!("{}/.", path("ticket"))).is_err());
        assert!(check_destination(&path(&"t".repeat(300))).is_err());
        std::fs::remove_dir_all(&directory)
    }

    #[test]
    fn a_ticket_is_published_once_and_no_staging_is_left() -> Result<(), Box<dyn std::error::Error>>
    {
        let base = std::env::temp_dir().join(format!("ma2a-invite-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base)?;
        let output = base.join("ticket");
        let path = output.to_str().ok_or("path")?;
        super::write_owner_only(path, b"secret")?;
        assert_eq!(std::fs::read(&output)?, b"secret\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&output)?.permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // An existing file is never replaced, and no staging entry is left either way.
        assert!(super::write_owner_only(path, b"other").is_err());
        assert_eq!(std::fs::read(&output)?, b"secret\n");
        let names = std::fs::read_dir(&base)?
            .map(|entry| Ok(entry?.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(names, ["ticket"]);
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
