//! Directory handles on Windows: every verified ancestor stays open without delete sharing.
//!
//! Windows cannot rename or delete a directory while a handle to it is open without
//! `FILE_SHARE_DELETE`. Holding every ancestor from the root down for the whole operation
//! therefore pins the path: nothing can swap an ancestor for a reparse point mid-operation.
//! Each handle is opened on the entry itself (`FILE_FLAG_OPEN_REPARSE_POINT`) and refused when
//! it is a reparse point.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use ma2a_core::service::{FileKind, ServiceError};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use super::entry::Entry;
use crate::{client::resume::is_private_directory, fs_error};

/// One pinned directory inside an exported root.
#[derive(Clone, Debug)]
pub(crate) struct Directory {
    path: PathBuf,
    held: Vec<Arc<File>>,
}

/// Opens `path` itself, sharing reads and writes but never deletion or renaming.
fn pin(path: &Path) -> Result<File, ServiceError> {
    let handle = fs_error::map(
        OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path),
    )?;
    let metadata = fs_error::map(handle.metadata())?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ServiceError::SymlinkRefused);
    }
    if !metadata.is_dir() {
        return Err(ServiceError::NotADirectory);
    }
    Ok(handle)
}

impl Directory {
    /// Opens the canonical root directory recorded at exposure time.
    pub(crate) fn open_root(path: &Path) -> Result<Self, ServiceError> {
        // The root was canonicalized when it was exposed, so it is pinned as a directory
        // without refusing a reparse point the owner deliberately exported through.
        let handle = fs_error::map(
            OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path),
        )?;
        Ok(Self {
            path: path.to_path_buf(),
            held: vec![Arc::new(handle)],
        })
    }

    /// Opens and pins a child directory, refusing a reparse point.
    pub(crate) fn child(&self, name: &str) -> Result<Self, ServiceError> {
        let path = self.path.join(name);
        let handle = pin(&path)?;
        let mut held = self.held.clone();
        held.push(Arc::new(handle));
        Ok(Self { path, held })
    }

    /// Returns the logical path of this directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn at(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Returns metadata of the directory itself.
    pub(crate) fn itself(&self) -> Result<Entry, ServiceError> {
        Ok(Entry::from_metadata(&fs_error::map(fs::metadata(
            &self.path,
        ))?))
    }

    /// Returns metadata of one child without following it, or `None` when absent.
    pub(crate) fn entry(&self, name: &str) -> Result<Option<Entry>, ServiceError> {
        match fs::symlink_metadata(self.at(name)) {
            Ok(metadata) => Ok(Some(Entry::from_metadata(&metadata))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(fs_error::service_error(&error)),
        }
    }

    /// Visits every child name; names that are not valid Unicode are passed as `None`.
    pub(crate) fn for_each_name(&self, mut visit: impl FnMut(Option<&str>)) -> io::Result<()> {
        for child in fs::read_dir(&self.path)? {
            visit(child?.file_name().to_str());
        }
        Ok(())
    }

    pub(crate) fn mkdir(&self, name: &str) -> io::Result<()> {
        fs::create_dir(self.at(name))
    }

    pub(crate) fn remove_file(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.at(name))
    }

    /// Removes a non-directory entry; a directory symbolic link is removed as a directory.
    pub(crate) fn remove_entry(&self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.at(name)) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                fs::remove_dir(self.at(name))
            }
            result => result,
        }
    }

    pub(crate) fn remove_dir(&self, name: &str) -> io::Result<()> {
        fs::remove_dir(self.at(name))
    }

    /// Removes a directory tree one entry at a time; a reparse point inside it, such as a
    /// junction or a directory link, is removed itself and never entered.
    ///
    /// A download's private directory inside the tree is kept with everything in it, so its
    /// progress survives; the directories above it then stay too, and the removal ends with
    /// `not_empty` after removing everything else.
    pub(crate) fn remove_tree(&self, name: &str) -> Result<(), ServiceError> {
        // Refuses a reparse point, and pins the directory while its entries are removed.
        let directory = self.child(name)?;
        for child in fs_error::map(fs::read_dir(&directory.path))? {
            let child = fs_error::map(child)?.file_name();
            // A name that is not valid Unicode cannot be addressed remotely; it keeps the
            // directory non-empty, as on Unix.
            let Some(child) = child.to_str() else {
                continue;
            };
            if is_private_directory(std::ffi::OsStr::new(child)) {
                continue;
            }
            match directory.entry(child)? {
                // A subdirectory left non-empty by a kept download does not stop its siblings.
                Some(entry) if entry.kind == FileKind::Directory => {
                    match directory.remove_tree(child) {
                        Err(ServiceError::NotEmpty) => {}
                        removed => removed?,
                    }
                }
                Some(_) => fs_error::map(directory.remove_entry(child))?,
                None => {}
            }
        }
        // The pin denies deletion, so it is released before the directory itself goes.
        drop(directory);
        fs_error::map(self.remove_dir(name))
    }

    /// Renames `from` in this directory to `to`, replacing an existing destination.
    pub(crate) fn rename(&self, from: &str, (to, name): (&Self, &str)) -> io::Result<()> {
        fs::rename(self.at(from), to.at(name))
    }

    /// Renames `from` to `to`, failing with `already_exists` instead of replacing it.
    pub(crate) fn rename_no_clobber(
        &self,
        from: &str,
        (to, name): (&Self, &str),
    ) -> Result<(), ServiceError> {
        super::rename_windows::no_clobber(&self.at(from), &to.at(name))
    }

    /// Links `from` under the new name `to` in this directory.
    pub(crate) fn hard_link(&self, from: &str, to: &str) -> io::Result<()> {
        fs::hard_link(self.at(from), self.at(to))
    }

    /// Opens an existing child for reading without following a reparse point.
    pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.at(name))?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        Ok(file)
    }

    /// Creates a child file exclusively; an existing entry or link is never reused.
    pub(crate) fn create_new(&self, name: &str) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.at(name))
    }

    /// Windows commits directory entries with the operation itself.
    #[expect(
        clippy::unused_self,
        reason = "the Unix counterpart syncs this directory's handle"
    )]
    pub(crate) const fn sync(&self) {}
}
