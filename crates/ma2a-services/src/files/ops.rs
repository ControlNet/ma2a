//! Content-free Files operations, run on a blocking thread.

use std::{collections::BinaryHeap, io::ErrorKind, path::Path};

use ma2a_core::service::{
    DirEntry, FileKind, FilesResult, MAX_LIST_PAGE, MAX_PATH_COMPONENTS, RemotePath, ServiceError,
};

use super::{
    resolve::{self, Directory},
    stop::Stop,
};
use crate::fs_error;

/// A failed operation and whether some of its effect may already have happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) error: ServiceError,
    pub(crate) partial: bool,
}

impl From<ServiceError> for Failure {
    fn from(error: ServiceError) -> Self {
        Self {
            error,
            partial: false,
        }
    }
}

const fn partial(error: ServiceError) -> Failure {
    Failure {
        error,
        partial: true,
    }
}

/// Returns metadata of one object without following a final link.
pub(crate) fn stat(root: &Path, path: &RemotePath) -> Result<FilesResult, ServiceError> {
    let entry = match resolve::parent(root, path)? {
        Some((parent, name)) => parent.entry(name)?.ok_or(ServiceError::NotFound)?,
        None => resolve::walk(root, &[])?.itself()?,
    };
    Ok(FilesResult::Metadata(entry.wire()))
}

/// Returns the byte-order-first page of names after `cursor`.
///
/// Memory stays bounded by the page: only the smallest `MAX_LIST_PAGE + 1` names are kept.
/// Names that cannot be addressed remotely are skipped, before paging, so every page and
/// cursor decodes and every listed name can be requested again: names that are not valid UTF-8,
/// names that are not a valid path component (control characters, `\`, `:`), and names longer
/// than a path component may be, which a Windows volume allows (255 UTF-16 units can take up to
/// 765 UTF-8 bytes).
pub(crate) fn list(
    root: &Path,
    path: &RemotePath,
    cursor: Option<&str>,
) -> Result<FilesResult, ServiceError> {
    let directory = resolve::directory(root, path)?;
    let mut smallest = BinaryHeap::with_capacity(MAX_LIST_PAGE + 1);
    fs_error::map(directory.for_each_name(|name| {
        let Some(name) = name.filter(|name| addressable(name)) else {
            return;
        };
        if cursor.is_some_and(|cursor| name <= cursor) {
            return;
        }
        smallest.push(name.to_owned());
        if smallest.len() > MAX_LIST_PAGE + 1 {
            smallest.pop();
        }
    }))?;
    let mut names = smallest.into_sorted_vec();
    let more = names.len() > MAX_LIST_PAGE;
    names.truncate(MAX_LIST_PAGE);
    let last_scanned = names.last().cloned();
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let Some(entry) = directory.entry(&name)? else {
            continue;
        };
        entries.push(DirEntry::new(name, entry.wire()));
    }
    let next = if more { last_scanned } else { None };
    Ok(FilesResult::Listing(entries, next))
}

/// Whether `name` can be named again in a request: it is a valid remote path component, which
/// also keeps it within what `Decoder::text` reads.
fn addressable(name: &str) -> bool {
    ma2a_core::service::valid_component(name)
}

/// Creates one directory, or every missing ancestor with `parents`.
pub(crate) fn mkdir(root: &Path, path: &RemotePath, parents: bool) -> Result<FilesResult, Failure> {
    if !parents {
        let (parent, name) = resolve::parent(root, path)?.ok_or(ServiceError::AlreadyExists)?;
        fs_error::map(parent.mkdir(name))?;
        return Ok(FilesResult::Done);
    }
    // Validates every component before anything is created.
    resolve::check(path)?;
    let mut current = resolve::walk(root, &[])?;
    let mut created = false;
    let fail = |error: ServiceError, created: bool| Failure {
        error,
        partial: created,
    };
    for component in path.components() {
        match current.mkdir(component) {
            Ok(()) => created = true,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(fail(fs_error::service_error(&error), created)),
        }
        current = current
            .child(component)
            .map_err(|error| fail(error, created))?;
    }
    Ok(FilesResult::Done)
}

/// Opens the parent of a path that must not be the root itself.
fn entry_parent<'path>(
    root: &Path,
    path: &'path RemotePath,
) -> Result<(Directory, &'path str), ServiceError> {
    resolve::parent(root, path)?.ok_or(ServiceError::InvalidPath)
}

/// Renames one object, including a symbolic link itself, within one root.
pub(crate) fn rename(
    root: &Path,
    (from, to): (&RemotePath, &RemotePath),
    replace: bool,
) -> Result<FilesResult, ServiceError> {
    let (source, source_name) = entry_parent(root, from)?;
    let (destination, destination_name) = entry_parent(root, to)?;
    source.entry(source_name)?.ok_or(ServiceError::NotFound)?;
    let to = (&destination, destination_name);
    if replace {
        fs_error::map(source.rename(source_name, to))?;
    } else {
        source.rename_no_clobber(source_name, to)?;
    }
    Ok(FilesResult::Done)
}

/// Removes a file, link, or empty directory; `recursive` removes a directory tree.
///
/// A recursive removal stops before its next change once `stop` is set, failing with
/// `cancelled` and reporting that part of the tree may already be gone.
pub(crate) fn remove(
    root: &Path,
    path: &RemotePath,
    (recursive, stop): (bool, &Stop),
) -> Result<FilesResult, Failure> {
    let (parent, name) = entry_parent(root, path)?;
    let entry = parent.entry(name)?.ok_or(ServiceError::NotFound)?;
    match entry.kind {
        FileKind::Directory if recursive => parent
            .remove_tree(name, (stop, MAX_PATH_COMPONENTS))
            .map_err(partial)?,
        FileKind::Directory => fs_error::map(parent.remove_dir(name))?,
        FileKind::File | FileKind::Symlink | FileKind::Other => {
            fs_error::map(parent.remove_entry(name))?;
        }
    }
    Ok(FilesResult::Done)
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{FilesResult, RemotePath, ServiceError};

    use super::{Failure, Stop, addressable, remove};

    #[test]
    fn a_stopped_recursive_removal_changes_nothing_more() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!("ma2a-stop-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tree/inner"))?;
        std::fs::write(root.join("tree/a"), b"a")?;
        std::fs::write(root.join("tree/inner/b"), b"b")?;
        let path = RemotePath::parse("docs:/tree")?;

        // Once its caller stopped waiting, the removal stops before its first change.
        let stopped = Stop::default();
        drop(stopped.on_drop());
        assert_eq!(
            remove(&root, &path, (true, &stopped)),
            Err(Failure {
                error: ServiceError::Cancelled,
                partial: true
            })
        );
        assert_eq!(std::fs::read(root.join("tree/a"))?, b"a");
        assert_eq!(std::fs::read(root.join("tree/inner/b"))?, b"b");

        // Otherwise the whole tree goes.
        assert_eq!(
            remove(&root, &path, (true, &Stop::default())),
            Ok(FilesResult::Done)
        );
        assert!(!root.join("tree").exists());
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_recursive_removal_keeps_what_lies_too_deep() -> Result<(), Box<dyn std::error::Error>> {
        // Given: a chain of directories deeper than a removal enters, with a file near the top
        // and one at the bottom.
        let root = std::env::temp_dir().join(format!("ma2a-deep-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&root);
        let deepest = (0..70).fold(root.join("tree"), |path, level| {
            path.join(level.to_string())
        });
        std::fs::create_dir_all(&deepest)?;
        std::fs::write(root.join("tree/0/near"), b"n")?;
        std::fs::write(deepest.join("far"), b"f")?;

        // When
        let removed = remove(
            &root,
            &RemotePath::parse("docs:/tree")?,
            (true, &Stop::default()),
        );

        // Then: what it reached is gone; the deep end stays, as a kept download would.
        assert_eq!(
            removed,
            Err(Failure {
                error: ServiceError::NotEmpty,
                partial: true
            })
        );
        assert!(!root.join("tree/0/near").exists());
        assert_eq!(std::fs::read(deepest.join("far"))?, b"f");
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_name_longer_than_a_path_component_is_not_addressable() {
        // 85 three-byte characters fill a component; 100 exceed it, though NTFS takes them.
        assert!(addressable(&"\u{6587}".repeat(85)));
        assert!(!addressable(&"\u{6587}".repeat(100)));
        assert!(addressable(&"n".repeat(255)));
        assert!(!addressable(&"n".repeat(256)));
        assert!(!addressable(""));
        assert!(!addressable("bad\nname"));
        assert!(!addressable("bad\tname"));
        assert!(!addressable("bad\x7fname"));
        assert!(!addressable("bad\u{0085}name"));
    }
}
