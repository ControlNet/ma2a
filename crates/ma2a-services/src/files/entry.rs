//! Platform-neutral metadata of one entry, read without following a final link.

use std::{fs::Metadata, time::UNIX_EPOCH};

use ma2a_core::service::{FileKind, FileMetadata, ServiceError};

/// What Files reports about one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified_ms: Option<u64>,
    pub(crate) read_only: bool,
}

impl Entry {
    /// Reads the metadata of an entry opened or listed without following it.
    pub(crate) fn from_metadata(metadata: &Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            FileKind::Symlink
        } else if file_type.is_dir() {
            FileKind::Directory
        } else if file_type.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
        Self {
            kind,
            len: metadata.len(),
            modified_ms,
            read_only: metadata.permissions().readonly(),
        }
    }

    /// Converts the entry into the bounded wire view.
    pub(crate) const fn wire(&self) -> FileMetadata {
        FileMetadata::new(self.kind, self.len)
            .with_modified_ms(self.modified_ms)
            .with_read_only(self.read_only)
    }

    /// Requires one kind; devices, sockets, and FIFOs are never opened.
    pub(crate) fn require(&self, wanted: FileKind) -> Result<(), ServiceError> {
        match (self.kind, wanted) {
            (FileKind::Symlink, _) => Err(ServiceError::SymlinkRefused),
            (kind, wanted) if kind == wanted => Ok(()),
            (FileKind::Directory, _) => Err(ServiceError::IsADirectory),
            (FileKind::File, _) => Err(ServiceError::NotADirectory),
            (FileKind::Other, _) => Err(ServiceError::InvalidPath),
        }
    }
}
