//! Temporary sibling files and their atomic finalization.

use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
};

use ma2a_core::service::{ServiceError, WriteMode};

use super::rename;
use crate::fs_error;

/// A temporary sibling that is removed on drop unless it was finalized.
#[derive(Debug)]
pub(crate) struct TempFile {
    path: PathBuf,
    file: Option<File>,
    finalized: bool,
}

impl TempFile {
    /// Creates `name` exclusively inside `parent`; it never replaces an existing entry.
    pub(crate) fn create(parent: &Path, name: &str) -> Result<Self, ServiceError> {
        let path = parent.join(name);
        let file = fs_error::map(OpenOptions::new().write(true).create_new(true).open(&path))?;
        Ok(Self {
            path,
            file: Some(file),
            finalized: false,
        })
    }

    /// Creates a uniquely named temporary sibling for one small write.
    pub(crate) fn unique(parent: &Path) -> Result<Self, ServiceError> {
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| ServiceError::Internal)?;
        Self::create(
            parent,
            &format!(".ma2a-write-{}.tmp", crate::report::hex(&nonce)),
        )
    }

    /// Appends bytes.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), ServiceError> {
        let file = self.file.as_mut().ok_or(ServiceError::Internal)?;
        fs_error::map(file.write_all(bytes))
    }

    /// Flushes content to stable storage and moves it to `destination`.
    pub(crate) fn finalize(
        mut self,
        destination: &Path,
        mode: WriteMode,
    ) -> Result<(), ServiceError> {
        let file = self.file.take().ok_or(ServiceError::Internal)?;
        fs_error::map(file.sync_all())?;
        drop(file);
        match mode {
            WriteMode::Replace => fs_error::map(fs::rename(&self.path, destination))?,
            WriteMode::CreateNew => create_new(&self.path, destination)?,
        }
        self.finalized = true;
        sync_parent(destination);
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.finalized {
            let _removed = fs::remove_file(&self.path);
        }
    }
}

/// Publishes `temporary` under `destination` only if nothing exists there.
///
/// A hard link claims the name atomically; filesystems without hard links fall back to the
/// platform no-clobber rename.
fn create_new(temporary: &Path, destination: &Path) -> Result<(), ServiceError> {
    match fs::hard_link(temporary, destination) {
        Ok(()) => {
            let _removed = fs::remove_file(temporary);
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(ServiceError::AlreadyExists),
        Err(_) => rename::no_clobber(temporary, destination),
    }
}

/// Makes the new directory entry durable.
#[cfg(unix)]
fn sync_parent(destination: &Path) {
    if let Some(parent) = destination.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _synced = directory.sync_all();
    }
}

/// Windows commits directory entries with the rename itself and cannot sync directories.
#[cfg(not(unix))]
const fn sync_parent(_destination: &Path) {}
