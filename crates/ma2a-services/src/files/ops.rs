//! Content-free Files operations, run on a blocking thread.

use std::{collections::BinaryHeap, io::ErrorKind, path::Path};

use ma2a_core::service::{
    DirEntry, FileKind, FilesResult, MAX_LIST_PAGE, MAX_PATH_COMPONENT_LEN, RemotePath,
    ServiceError,
};

use super::resolve::{self, Directory};
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
/// cursor decodes: names that are not valid UTF-8, and names longer than a path component may
/// be, which a Windows volume allows (255 UTF-16 units can take up to 765 UTF-8 bytes).
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
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let Some(entry) = directory.entry(&name)? else {
            continue;
        };
        entries.push(DirEntry::new(name, entry.wire()));
    }
    let next = if more {
        entries.last().map(|entry| entry.name.clone())
    } else {
        None
    };
    Ok(FilesResult::Listing(entries, next))
}

/// Whether `name` fits in one remote path component.
const fn addressable(name: &str) -> bool {
    name.len() <= MAX_PATH_COMPONENT_LEN
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
pub(crate) fn remove(
    root: &Path,
    path: &RemotePath,
    recursive: bool,
) -> Result<FilesResult, Failure> {
    let (parent, name) = entry_parent(root, path)?;
    let entry = parent.entry(name)?.ok_or(ServiceError::NotFound)?;
    match entry.kind {
        FileKind::Directory if recursive => parent.remove_tree(name).map_err(partial)?,
        FileKind::Directory => fs_error::map(parent.remove_dir(name))?,
        FileKind::File | FileKind::Symlink | FileKind::Other => {
            fs_error::map(parent.remove_entry(name))?;
        }
    }
    Ok(FilesResult::Done)
}

#[cfg(test)]
mod tests {
    use super::addressable;

    #[test]
    fn a_name_longer_than_a_path_component_is_not_addressable() {
        // 85 three-byte characters fill a component; 100 exceed it, though NTFS takes them.
        assert!(addressable(&"\u{6587}".repeat(85)));
        assert!(!addressable(&"\u{6587}".repeat(100)));
        assert!(addressable(&"n".repeat(255)));
        assert!(!addressable(&"n".repeat(256)));
    }
}
