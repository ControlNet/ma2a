//! Root-relative path resolution that never follows a symbolic link.
//!
//! Every intermediate component is checked with `lstat`; concurrent modification by other
//! same-user local processes is outside the threat model.

use std::{
    fs::{self, Metadata},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ma2a_core::service::{FileKind, FileMetadata, RemotePath, ServiceError};

use crate::fs_error;

/// Returns whether a component cannot name a regular entry on Windows hosts.
pub(crate) fn is_windows_reserved(component: &str) -> bool {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    if component.ends_with('.') || component.ends_with(' ') {
        return true;
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end()
        .to_ascii_uppercase();
    if DEVICES.contains(&stem.as_str()) {
        return true;
    }
    let bytes = stem.as_bytes();
    matches!(bytes, [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] if (b'1'..=b'9').contains(digit))
}

/// Joins the root-relative components after rejecting host-invalid names.
pub(crate) fn join(root: &Path, path: &RemotePath) -> Result<PathBuf, ServiceError> {
    let mut joined = root.to_path_buf();
    for component in path.components() {
        if cfg!(windows) && is_windows_reserved(component) {
            return Err(ServiceError::InvalidPath);
        }
        joined.push(component);
    }
    Ok(joined)
}

/// Resolves `path` and verifies that every ancestor below the root is a real directory.
pub(crate) fn resolve(root: &Path, path: &RemotePath) -> Result<PathBuf, ServiceError> {
    let target = join(root, path)?;
    let mut current = root.to_path_buf();
    let Some((_last, ancestors)) = path.components().split_last() else {
        return Ok(target);
    };
    for component in ancestors {
        current.push(component);
        let metadata = fs_error::map(fs::symlink_metadata(&current))?;
        if metadata.file_type().is_symlink() {
            return Err(ServiceError::SymlinkRefused);
        }
        if !metadata.is_dir() {
            return Err(ServiceError::NotADirectory);
        }
    }
    Ok(target)
}

/// Returns final-component metadata without following it, or `None` when absent.
pub(crate) fn lstat(path: &Path) -> Result<Option<Metadata>, ServiceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fs_error::service_error(&error)),
    }
}

/// Resolves an existing directory, refusing a final symbolic link.
pub(crate) fn directory(root: &Path, path: &RemotePath) -> Result<PathBuf, ServiceError> {
    let target = resolve(root, path)?;
    let metadata = lstat(&target)?.ok_or(ServiceError::NotFound)?;
    kind_is(&metadata, FileKind::Directory)?;
    Ok(target)
}

/// Resolves an existing regular file, refusing a final symbolic link.
pub(crate) fn regular_file(
    root: &Path,
    path: &RemotePath,
) -> Result<(PathBuf, Metadata), ServiceError> {
    let target = resolve(root, path)?;
    let metadata = lstat(&target)?.ok_or(ServiceError::NotFound)?;
    kind_is(&metadata, FileKind::File)?;
    Ok((target, metadata))
}

/// Resolves the parent directory of a destination that may not exist yet.
pub(crate) fn destination(
    root: &Path,
    path: &RemotePath,
) -> Result<(PathBuf, PathBuf, Option<Metadata>), ServiceError> {
    if path.is_root() {
        return Err(ServiceError::InvalidPath);
    }
    let target = resolve(root, path)?;
    let parent = target
        .parent()
        .ok_or(ServiceError::InvalidPath)?
        .to_path_buf();
    let existing = lstat(&target)?;
    if let Some(metadata) = &existing {
        kind_is(metadata, FileKind::File)?;
    }
    Ok((target, parent, existing))
}

/// Requires one kind; devices, sockets, and FIFOs are never opened.
fn kind_is(metadata: &Metadata, wanted: FileKind) -> Result<(), ServiceError> {
    match (file_kind(metadata), wanted) {
        (FileKind::Symlink, _) => Err(ServiceError::SymlinkRefused),
        (kind, wanted) if kind == wanted => Ok(()),
        (FileKind::Directory, _) => Err(ServiceError::IsADirectory),
        (FileKind::File, _) => Err(ServiceError::NotADirectory),
        (FileKind::Other, _) => Err(ServiceError::InvalidPath),
    }
}

/// Classifies metadata obtained without following a final link.
pub(crate) fn file_kind(metadata: &Metadata) -> FileKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

/// Converts metadata into the bounded wire view.
pub(crate) fn wire_metadata(metadata: &Metadata) -> FileMetadata {
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
    FileMetadata::new(file_kind(metadata), metadata.len())
        .with_modified_ms(modified_ms)
        .with_read_only(metadata.permissions().readonly())
}

#[cfg(test)]
mod tests {
    use super::is_windows_reserved;

    #[test]
    fn windows_device_names_and_trailing_dots_are_reserved() {
        for reserved in ["CON", "nul.txt", "Com1", "lpt9.log", "a.", "b "] {
            assert!(is_windows_reserved(reserved), "{reserved}");
        }
        for ordinary in ["console", "com10", "lpt0", "a.b", "nul_", "COM"] {
            assert!(!is_windows_reserved(ordinary), "{ordinary}");
        }
    }
}
