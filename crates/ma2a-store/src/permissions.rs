use std::{fs, path::Path};

use crate::StoreError;

pub(crate) fn ensure_private_dir(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_local_absolute(path)?;
    #[cfg(unix)]
    let cleaned = clean_unix_path(path);
    #[cfg(unix)]
    let path = cleaned.as_path();
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(path, target)?;
        }
        Err(error) => return Err(error.into()),
    }
    validate_private_path(path, target, PathKind::Directory)
}

/// Makes sure `path`, an absolute local path, is a directory only this account may enter:
/// creates it so when missing, and otherwise accepts it only when it already is one.
///
/// # Errors
/// Returns [`StoreError::InsecurePermissions`] for an existing directory that is not private.
pub fn ensure_private_directory(path: &Path, target: &'static str) -> Result<(), StoreError> {
    ensure_private_dir(path, target)
}

/// Restricts `path`, a directory the caller just created, to the current account (and SYSTEM
/// on Windows, under a protected ACL its children inherit), then confirms that it is.
///
/// # Errors
/// Returns [`StoreError`] when the permissions cannot be applied or confirmed.
pub fn protect_private_directory(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_local_absolute(path)?;
    #[cfg(unix)]
    let cleaned = clean_unix_path(path);
    #[cfg(unix)]
    let path = cleaned.as_path();
    refuse_symlink(path, target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    apply_windows_acl(path, target, PathKind::Directory)?;
    validate_private_path(path, target, PathKind::Directory)
}

/// Creates `path`, which must not exist yet, as a directory only this account may enter.
///
/// # Errors
/// Returns [`StoreError::Io`] with [`std::io::ErrorKind::AlreadyExists`] when anything
/// already has that name, which is left as it was.
pub fn create_private_directory(path: &Path) -> Result<(), StoreError> {
    create_new_private_dir(path)
}

pub(crate) fn create_new_private_dir(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        fs::DirBuilder::new().mode(0o700).create(path)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        fs::create_dir(path)?;
        apply_windows_acl(path, "private directory", PathKind::Directory)
    }
}

/// Restricts `path`, a regular file the caller just created, to the current account (and
/// SYSTEM on Windows, with no inherited entries), then confirms that it is.
///
/// # Errors
/// Returns [`StoreError`] when the permissions cannot be applied or confirmed.
pub fn protect_private_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    protect_new_file(path, target)
}

pub(crate) fn protect_new_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    refuse_symlink(path, target)?;
    protect_file(path, target)?;
    validate_private_path(path, target, PathKind::File)
}

/// Changing permissions follows a symlink, so a link is refused before anything is changed.
fn refuse_symlink(path: &Path, target: &'static str) -> Result<(), StoreError> {
    #[cfg(unix)]
    let cleaned = clean_unix_path(path);
    #[cfg(unix)]
    let path = cleaned.as_path();
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is a symlink or has the wrong file type",
        });
    }
    Ok(())
}

/// Removes trailing separators and `.` components on Unix so `lstat` inspects the path entry
/// itself rather than following a final symlink.
#[cfg(unix)]
fn clean_unix_path(path: &Path) -> std::path::PathBuf {
    path.components().collect()
}

pub(crate) fn validate_private_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_private_path(path, target, PathKind::File)
}

fn validate_local_absolute(path: &Path) -> Result<(), StoreError> {
    if !is_local_absolute(path) {
        return Err(StoreError::InvalidStateDirectory);
    }
    Ok(())
}

/// Whether `path` is absolute and names a local volume, the only state path the Store accepts.
#[must_use]
pub fn is_local_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        && !is_remote(path)
}

#[cfg(unix)]
fn create_private_dir(path: &Path, _target: &'static str) -> Result<(), StoreError> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_dir(path: &Path, target: &'static str) -> Result<(), StoreError> {
    fs::create_dir_all(path)?;
    apply_windows_acl(path, target, PathKind::Directory)
}

#[cfg(unix)]
fn protect_file(path: &Path, _target: &'static str) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn protect_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    apply_windows_acl(path, target, PathKind::File)
}

#[cfg(unix)]
fn validate_private_path(
    path: &Path,
    target: &'static str,
    kind: PathKind,
) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let cleaned = clean_unix_path(path);
    let metadata = fs::symlink_metadata(&cleaned)?;
    check_type(&metadata, target, kind)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is not owned by the current user",
        });
    }
    let expected_mode = match kind {
        PathKind::Directory => 0o700,
        PathKind::File => 0o600,
    };
    if metadata.permissions().mode() & 0o777 != expected_mode {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "mode must be exactly owner-only",
        });
    }
    Ok(())
}

/// Refuses a symlink, or anything but the kind of object expected.
fn check_type(
    metadata: &fs::Metadata,
    target: &'static str,
    kind: PathKind,
) -> Result<(), StoreError> {
    let expected_type = match kind {
        PathKind::Directory => metadata.is_dir(),
        PathKind::File => metadata.is_file(),
    };
    if metadata.file_type().is_symlink() || !expected_type {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is a symlink or has the wrong file type",
        });
    }
    Ok(())
}

/// Checks the object itself through one handle, so a link put in its place is seen as a link.
#[cfg(windows)]
fn validate_private_path(
    path: &Path,
    target: &'static str,
    kind: PathKind,
) -> Result<(), StoreError> {
    let file = windows_acl::open(path, false)?;
    check_type(&file.metadata()?, target, kind)?;
    if !windows_acl::is_private(&file).unwrap_or(false) {
        return Err(StoreError::WindowsAcl {
            operation: "validation",
        });
    }
    Ok(())
}

/// Protects the object `path` names, after checking through the same handle that it is not a
/// link or another kind of object put there since the caller looked.
#[cfg(windows)]
fn apply_windows_acl(path: &Path, target: &'static str, kind: PathKind) -> Result<(), StoreError> {
    use windows_sys::Win32::Security::{CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE};

    let inherit = match kind {
        PathKind::Directory => CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
        PathKind::File => 0,
    };
    let failed = |_: std::io::Error| StoreError::WindowsAcl {
        operation: "application",
    };
    let file = windows_acl::open(path, true).map_err(failed)?;
    check_type(&file.metadata()?, target, kind)?;
    windows_acl::protect(&file, inherit).map_err(failed)
}

#[cfg(not(windows))]
const fn is_remote(_path: &Path) -> bool {
    false
}

/// Only a drive letter names a local volume for certain: `\\?\GLOBALROOT\Device\Mup\…` and
/// other verbatim or device paths can reach a network share as well as a UNC path can. A drive
/// letter can be mapped to a network share too, and a directory link on a local drive can point
/// at one, so the nearest level that exists, with every link resolved, must be on a local drive
/// as well. A link counts as existing even when its target cannot be reached. Whatever cannot be
/// resolved, or read, counts as remote.
#[cfg(windows)]
fn is_remote(path: &Path) -> bool {
    if !is_on_local_drive(path) {
        return true;
    }
    for level in path.ancestors() {
        match fs::symlink_metadata(level) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return true,
            Ok(_) => return !fs::canonicalize(level).is_ok_and(|real| is_on_local_drive(&real)),
        }
    }
    true
}

/// Whether `path` starts with the letter of a drive that is not a network share.
#[cfg(windows)]
fn is_on_local_drive(path: &Path) -> bool {
    use std::path::{Component, Prefix};

    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(
            prefix.kind(),
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)
                if !windows_acl::is_remote_drive(letter)
        )
    )
}

#[derive(Clone, Copy)]
enum PathKind {
    Directory,
    File,
}

#[cfg(windows)]
mod windows_acl;
#[cfg(all(test, windows))]
mod windows_acl_tests;
#[cfg(windows)]
mod windows_sid;
