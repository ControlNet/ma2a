//! Temporary sibling files and their atomic finalization.

use std::{
    fs::File,
    io::{ErrorKind, Write as _},
};

use ma2a_core::service::{ServiceError, WriteMode};

use super::resolve::Directory;
use crate::fs_error;

/// A temporary file in a held directory that is removed on drop unless it was finalized.
#[derive(Debug)]
pub(crate) struct TempFile {
    directory: Directory,
    name: String,
    file: Option<File>,
    finalized: bool,
}

impl TempFile {
    /// Creates `name` exclusively inside `directory`; it never replaces an existing entry.
    pub(crate) fn create(directory: Directory, name: &str) -> Result<Self, ServiceError> {
        let file = fs_error::map(directory.create_new(name))?;
        Ok(Self {
            directory,
            name: name.to_owned(),
            file: Some(file),
            finalized: false,
        })
    }

    /// Creates a uniquely named temporary sibling for one small write.
    pub(crate) fn unique(directory: Directory) -> Result<Self, ServiceError> {
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| ServiceError::Internal)?;
        Self::create(
            directory,
            &format!(".ma2a-write-{}.tmp", crate::report::hex(&nonce)),
        )
    }

    /// Appends bytes.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), ServiceError> {
        let file = self.file.as_mut().ok_or(ServiceError::Internal)?;
        fs_error::map(file.write_all(bytes))
    }

    /// Flushes content to stable storage and moves it to `destination` in the same directory.
    pub(crate) fn finalize(
        mut self,
        destination: &str,
        mode: WriteMode,
    ) -> Result<(), ServiceError> {
        let file = self.file.take().ok_or(ServiceError::Internal)?;
        fs_error::map(file.sync_all())?;
        drop(file);
        let directory = &self.directory;
        match mode {
            WriteMode::Replace => {
                fs_error::map(directory.rename(&self.name, (directory, destination)))?;
            }
            WriteMode::CreateNew => create_new(directory, (&self.name, destination))?,
        }
        self.finalized = true;
        directory.sync();
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.finalized {
            let _removed = self.directory.remove_file(&self.name);
        }
    }
}

/// Publishes `temporary` under `destination` only if nothing exists there.
///
/// A hard link claims the name atomically; filesystems without hard links fall back to the
/// platform no-clobber rename.
fn create_new(
    directory: &Directory,
    (temporary, destination): (&str, &str),
) -> Result<(), ServiceError> {
    match directory.hard_link(temporary, destination) {
        Ok(()) => {
            let _removed = directory.remove_file(temporary);
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(ServiceError::AlreadyExists),
        Err(_) => directory.rename_no_clobber(temporary, (directory, destination)),
    }
}
