//! Root-relative path resolution through held directory handles.
//!
//! Every component below the root is opened from its parent's handle without following a link
//! (see `dir_unix.rs` and `dir_windows.rs`), and the final step runs relative to the held
//! parent. A concurrent rename, including one a remote caller issues itself, therefore cannot
//! redirect an operation outside its exported root.

use std::{fs::File, path::Path};

use ma2a_core::service::{FileKind, RemotePath, ServiceError};

use super::entry::Entry;
pub(crate) use super::platform::Directory;
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

/// Name prefixes of the staging files the Runtime itself creates in an exported directory.
const RUNTIME_PREFIXES: [&str; 2] = [".ma2a-upload-", ".ma2a-write-"];

/// Returns whether a component names a Runtime staging file, which no caller may address:
/// an upload is published from the file it verified, never from one a caller replaced.
///
/// The prefixes are compared without regard to ASCII case, because a case-insensitive volume
/// (NTFS, APFS by default) resolves another spelling to the same entry.
pub(crate) fn is_runtime_reserved(component: &str) -> bool {
    RUNTIME_PREFIXES.iter().any(|prefix| {
        component
            .as_bytes()
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
    })
}

/// Rejects every component this host cannot name or the Runtime reserves.
pub(crate) fn check(path: &RemotePath) -> Result<(), ServiceError> {
    if path
        .components()
        .iter()
        .any(|name| is_runtime_reserved(name) || (cfg!(windows) && is_windows_reserved(name)))
    {
        return Err(ServiceError::InvalidPath);
    }
    Ok(())
}

/// Opens `root` and then each of `components` as a directory.
pub(crate) fn walk(root: &Path, components: &[String]) -> Result<Directory, ServiceError> {
    let mut current = Directory::open_root(root)?;
    for component in components {
        current = current.child(component)?;
    }
    Ok(current)
}

/// Opens the parent of a non-root path, returning it with the final component.
pub(crate) fn parent<'path>(
    root: &Path,
    path: &'path RemotePath,
) -> Result<Option<(Directory, &'path str)>, ServiceError> {
    check(path)?;
    let Some((last, ancestors)) = path.components().split_last() else {
        return Ok(None);
    };
    Ok(Some((walk(root, ancestors)?, last.as_str())))
}

/// Opens an existing directory, refusing any symbolic link on the way.
pub(crate) fn directory(root: &Path, path: &RemotePath) -> Result<Directory, ServiceError> {
    check(path)?;
    walk(root, path.components())
}

/// Opens an existing regular file for reading, refusing a final link or special file.
pub(crate) fn regular_file(
    root: &Path,
    path: &RemotePath,
) -> Result<(File, std::fs::Metadata), ServiceError> {
    let (parent, name) = parent(root, path)?.ok_or(ServiceError::IsADirectory)?;
    parent
        .entry(name)?
        .ok_or(ServiceError::NotFound)?
        .require(FileKind::File)?;
    let file = fs_error::map(parent.open_file(name))?;
    let metadata = fs_error::map(file.metadata())?;
    // The entry may have been replaced between the check and the open.
    Entry::from_metadata(&metadata).require(FileKind::File)?;
    Ok((file, metadata))
}

/// Opens the parent of a destination that may not exist yet; an existing one must be a file.
pub(crate) fn destination<'path>(
    root: &Path,
    path: &'path RemotePath,
) -> Result<(Directory, &'path str, Option<Entry>), ServiceError> {
    let (parent, name) = parent(root, path)?.ok_or(ServiceError::InvalidPath)?;
    let existing = parent.entry(name)?;
    if let Some(entry) = &existing {
        entry.require(FileKind::File)?;
    }
    Ok((parent, name, existing))
}

#[cfg(test)]
mod tests {
    use super::{is_runtime_reserved, is_windows_reserved};

    #[test]
    fn runtime_staging_names_cannot_be_addressed() -> Result<(), Box<dyn std::error::Error>> {
        let staged = ma2a_core::service::RemotePath::parse(
            "data:/inbox/.ma2a-upload-00112233445566778899aabbccddeeff.part",
        )?;
        assert_eq!(
            super::check(&staged),
            Err(ma2a_core::service::ServiceError::InvalidPath)
        );
        assert!(is_runtime_reserved(".ma2a-write-0011.tmp"));
        // Another spelling names the same entry on a case-insensitive volume.
        assert!(is_runtime_reserved(".MA2A-Upload-0011.part"));
        assert!(is_runtime_reserved(".Ma2a-WRITE-0011.tmp"));
        for ordinary in [
            ".ma2a-uploads",
            "ma2a-upload-1.part",
            ".ma2a",
            "report.part",
        ] {
            assert!(!is_runtime_reserved(ordinary), "{ordinary}");
        }
        assert_eq!(
            super::check(&ma2a_core::service::RemotePath::parse(
                "data:/inbox/report.part"
            )?),
            Ok(())
        );
        Ok(())
    }

    /// A caller may rename entries while another of its operations runs. Once an ancestor was
    /// opened, swapping its name for a link to outside the root must not redirect the rest.
    #[cfg(unix)]
    #[test]
    fn a_swapped_ancestor_never_redirects_a_held_operation()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = std::env::temp_dir().join(format!("ma2a-swap-{}", std::process::id()));
        let (root, outside) = (scratch.join("root"), scratch.join("outside"));
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("secret"), b"outside")?;
        let held = super::walk(&root, &["docs".to_owned()])?;

        std::fs::rename(root.join("docs"), root.join("moved"))?;
        std::os::unix::fs::symlink(&outside, root.join("docs"))?;

        assert_eq!(held.entry("secret")?, None);
        held.create_new("created")?;
        assert!(root.join("moved/created").exists());
        assert!(!outside.join("created").exists());
        assert_eq!(
            super::walk(&root, &["docs".to_owned()]).err(),
            Some(ma2a_core::service::ServiceError::SymlinkRefused)
        );
        std::fs::remove_dir_all(&scratch)?;
        Ok(())
    }

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
