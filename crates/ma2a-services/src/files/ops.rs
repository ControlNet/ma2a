//! Content-free Files operations, run on a blocking thread.

use std::{collections::BinaryHeap, fs, io::ErrorKind, path::Path};

use ma2a_core::service::{
    DirEntry, FileKind, FilesResult, MAX_LIST_PAGE, RemotePath, ServiceError,
};

use super::{
    rename,
    resolve::{self, lstat, wire_metadata},
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
    let target = resolve::resolve(root, path)?;
    let metadata = lstat(&target)?.ok_or(ServiceError::NotFound)?;
    Ok(FilesResult::Metadata(wire_metadata(&metadata)))
}

/// Returns the byte-order-first page of names after `cursor`.
///
/// Memory stays bounded by the page: only the smallest `MAX_LIST_PAGE + 1` names are kept.
/// Names that are not valid UTF-8 cannot be addressed remotely and are skipped.
pub(crate) fn list(
    root: &Path,
    path: &RemotePath,
    cursor: Option<&str>,
) -> Result<FilesResult, ServiceError> {
    let directory = resolve::directory(root, path)?;
    let mut smallest = BinaryHeap::with_capacity(MAX_LIST_PAGE + 1);
    for entry in fs_error::map(fs::read_dir(&directory))? {
        let Ok(name) = fs_error::map(entry)?.file_name().into_string() else {
            continue;
        };
        if cursor.is_some_and(|cursor| name.as_str() <= cursor) {
            continue;
        }
        smallest.push(name);
        if smallest.len() > MAX_LIST_PAGE + 1 {
            smallest.pop();
        }
    }
    let mut names = smallest.into_sorted_vec();
    let more = names.len() > MAX_LIST_PAGE;
    names.truncate(MAX_LIST_PAGE);
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let Some(metadata) = lstat(&directory.join(&name))? else {
            continue;
        };
        entries.push(DirEntry::new(name, wire_metadata(&metadata)));
    }
    let next = if more {
        entries.last().map(|entry| entry.name.clone())
    } else {
        None
    };
    Ok(FilesResult::Listing(entries, next))
}

/// Creates one directory, or every missing ancestor with `parents`.
pub(crate) fn mkdir(root: &Path, path: &RemotePath, parents: bool) -> Result<FilesResult, Failure> {
    if path.is_root() {
        return if parents {
            Ok(FilesResult::Done)
        } else {
            Err(ServiceError::AlreadyExists.into())
        };
    }
    if !parents {
        let target = resolve::resolve(root, path)?;
        fs_error::map(fs::create_dir(target))?;
        return Ok(FilesResult::Done);
    }
    resolve::join(root, path)?;
    let mut current = root.to_path_buf();
    let mut created = false;
    let fail = |error: ServiceError, created: bool| Failure {
        error,
        partial: created,
    };
    for component in path.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => created = true,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let metadata = lstat(&current)
                    .map_err(|error| fail(error, created))?
                    .ok_or_else(|| fail(ServiceError::Io, created))?;
                match resolve::file_kind(&metadata) {
                    FileKind::Directory => {}
                    FileKind::Symlink => return Err(fail(ServiceError::SymlinkRefused, created)),
                    FileKind::File | FileKind::Other => {
                        return Err(fail(ServiceError::NotADirectory, created));
                    }
                }
            }
            Err(error) => return Err(fail(fs_error::service_error(&error), created)),
        }
    }
    Ok(FilesResult::Done)
}

/// Renames one object, including a symbolic link itself, within one root.
pub(crate) fn rename(
    root: &Path,
    (from, to): (&RemotePath, &RemotePath),
    replace: bool,
) -> Result<FilesResult, ServiceError> {
    if from.is_root() || to.is_root() {
        return Err(ServiceError::InvalidPath);
    }
    let source = resolve::resolve(root, from)?;
    lstat(&source)?.ok_or(ServiceError::NotFound)?;
    let destination = resolve::resolve(root, to)?;
    if replace {
        fs_error::map(fs::rename(&source, &destination))?;
    } else {
        rename::no_clobber(&source, &destination)?;
    }
    Ok(FilesResult::Done)
}

/// Removes a file, link, or empty directory; `recursive` removes a directory tree.
pub(crate) fn remove(
    root: &Path,
    path: &RemotePath,
    recursive: bool,
) -> Result<FilesResult, Failure> {
    if path.is_root() {
        return Err(ServiceError::InvalidPath.into());
    }
    let target = resolve::resolve(root, path)?;
    let metadata = lstat(&target)?.ok_or(ServiceError::NotFound)?;
    match resolve::file_kind(&metadata) {
        FileKind::Directory if recursive => {
            fs::remove_dir_all(&target)
                .map_err(|error| partial(fs_error::service_error(&error)))?;
        }
        FileKind::Directory => fs_error::map(fs::remove_dir(&target))?,
        FileKind::File | FileKind::Symlink | FileKind::Other => {
            fs_error::map(remove_entry(&target))?;
        }
    }
    Ok(FilesResult::Done)
}

/// Removes a non-directory entry; a Windows directory symlink is removed as a directory.
fn remove_entry(target: &Path) -> std::io::Result<()> {
    match fs::remove_file(target) {
        Err(error) if cfg!(windows) && error.kind() == ErrorKind::PermissionDenied => {
            fs::remove_dir(target)
        }
        result => result,
    }
}
