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
///
/// The directory is opened once and every step runs relative to that handle, so all of them
/// happen in the same directory: one renamed meanwhile receives the ticket under its new name,
/// and no staging file is left behind in it.
#[cfg(unix)]
pub(super) fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    check_destination(path)?;
    let (directory, name) = destination_directory(path)?;
    let staged = stage(&directory, contents)?;
    publish_staged(&directory, &staged, name)
}

/// Opens the directory `path` names its file in, and returns it with that file name.
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
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
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

    #[cfg(unix)]
    #[test]
    fn a_ticket_stays_in_its_directory_when_the_directory_is_renamed()
    -> Result<(), Box<dyn std::error::Error>> {
        // Given: the ticket is staged in its output directory.
        let base = std::env::temp_dir().join(format!("ma2a-invite-moved-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        let (output, retired) = (base.join("output"), base.join("retired"));
        std::fs::create_dir_all(&output)?;
        let path = output.join("ticket");
        let path = path.to_str().ok_or("path")?;
        let (directory, name) = super::destination_directory(path)?;
        let staged = super::stage(&directory, b"secret")?;

        // When: the directory is rotated away and an empty one takes its name before publication.
        std::fs::rename(&output, &retired)?;
        std::fs::create_dir(&output)?;
        let published = super::publish_staged(&directory, &staged, name);

        // Then: the ticket is published in the directory it was staged in, with no staging left.
        let names = |directory: &std::path::Path| {
            std::fs::read_dir(directory)?
                .map(|entry| Ok(entry?.file_name()))
                .collect::<std::io::Result<Vec<_>>>()
        };
        let (moved, replacement) = (names(&retired)?, names(&output)?);
        let ticket = std::fs::read(retired.join("ticket"));
        std::fs::remove_dir_all(&base)?;
        assert!(published.is_ok(), "{published:?}");
        assert_eq!(moved, ["ticket"]);
        assert!(replacement.is_empty(), "{replacement:?}");
        assert_eq!(ticket?, b"secret\n");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publishing_claims_a_free_name_and_never_replaces_a_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("ma2a-invite-publish-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base)?;
        let destination = base.join("ticket");
        let path = destination.to_str().ok_or("path")?;
        let (directory, name) = super::destination_directory(path)?;
        let staged = super::stage(&directory, b"secret")?;
        super::publish_staged(&directory, &staged, name)?;
        assert_eq!(std::fs::read(&destination)?, b"secret\n");

        // A file someone else put at the name stays, whether linked or renamed over.
        std::fs::remove_file(&destination)?;
        std::fs::write(&destination, b"theirs")?;
        let staged = super::stage(&directory, b"secret")?;
        assert!(super::rename_no_clobber(&directory, (&staged, name)).is_err());
        assert!(super::publish_staged(&directory, &staged, name).is_err());
        assert_eq!(std::fs::read(&destination)?, b"theirs");
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
