//! Content-free Files operations, run on a blocking thread.

use std::{collections::BTreeSet, io::ErrorKind, ops::ControlFlow, path::Path};

use ma2a_core::service::{
    DirEntry, FileKind, FilesResult, MAX_LIST_PAGE, MAX_PATH_COMPONENTS, MAX_REMOTE_PATH_LEN,
    RemotePath, ServiceError,
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
    (cursor, stop): (Option<&str>, &Stop),
) -> Result<FilesResult, ServiceError> {
    let directory = resolve::directory(root, path)?;
    let budget = (
        path.components().len(),
        path.components().iter().map(String::len).sum::<usize>(),
    );
    let mut smallest = Smallest::default();
    // A listing nobody waits for any more stops instead of visiting the rest of the directory.
    fs_error::map(directory.for_each_name(|name| {
        if stop.is_set() {
            return ControlFlow::Break(());
        }
        let Some(name) = name.filter(|name| addressable(name, budget)) else {
            return ControlFlow::Continue(());
        };
        if cursor.is_some_and(|cursor| name <= cursor) {
            return ControlFlow::Continue(());
        }
        smallest.offer(name);
        ControlFlow::Continue(())
    }))?;
    if stop.is_set() {
        return Err(ServiceError::Cancelled);
    }
    let (names, more) = smallest.page();
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

/// The byte-order-smallest names seen, each once, and one more than a page so that a next page
/// shows: a directory read while it changes can yield a name twice.
#[derive(Default)]
struct Smallest(BTreeSet<String>);

impl Smallest {
    fn offer(&mut self, name: &str) {
        if self.0.insert(name.to_owned()) && self.0.len() > MAX_LIST_PAGE + 1 {
            self.0.pop_last();
        }
    }

    /// The page in byte order, and whether more names follow it.
    fn page(self) -> (Vec<String>, bool) {
        let mut names: Vec<String> = self.0.into_iter().collect();
        let more = names.len() > MAX_LIST_PAGE;
        names.truncate(MAX_LIST_PAGE);
        (names, more)
    }
}

/// Whether `name`, in a directory of `(components, bytes)`, can be named again in a request: it
/// is a valid remote path component, which also keeps it within what `Decoder::text` reads, the
/// Runtime does not reserve it, and its path stays within the path bounds.
fn addressable(name: &str, (components, bytes): (usize, usize)) -> bool {
    ma2a_core::service::valid_component(name)
        && !resolve::is_runtime_reserved(name)
        && !(cfg!(windows) && resolve::is_windows_reserved(name))
        && components < MAX_PATH_COMPONENTS
        && bytes.saturating_add(name.len()) <= MAX_REMOTE_PATH_LEN
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
#[path = "ops_tests.rs"]
mod tests;
