//! Small bounded reads and atomic small writes.

use std::{
    io::{Read as _, Seek as _, SeekFrom},
    path::Path,
};

use ma2a_core::service::{RemotePath, ServiceError, WriteMode};

use super::{commit::TempFile, resolve};
use crate::fs_error;

/// Opens an existing regular file for reading without following a final link.
pub(crate) fn open_regular(
    root: &Path,
    path: &RemotePath,
) -> Result<(std::fs::File, std::fs::Metadata), ServiceError> {
    resolve::regular_file(root, path)
}

/// Reads at most `length` bytes from `offset`, returning the bytes, file size, and offset.
pub(crate) fn read(
    root: &Path,
    path: &RemotePath,
    (offset, length): (u64, u32),
) -> Result<(Vec<u8>, u64, u64), super::Failure> {
    let (mut file, metadata) = open_regular(root, path)?;
    fs_error::map(file.seek(SeekFrom::Start(offset)))?;
    let mut bytes = Vec::new();
    fs_error::map(file.take(u64::from(length)).read_to_end(&mut bytes))?;
    Ok((bytes, metadata.len(), offset))
}

/// Validates a small-write destination before any content is accepted.
pub(crate) fn prepare_write(
    root: &Path,
    path: &RemotePath,
    mode: WriteMode,
) -> Result<(String, TempFile), ServiceError> {
    let (parent, name, existing) = resolve::destination(root, path)?;
    if mode == WriteMode::CreateNew && existing.is_some() {
        return Err(ServiceError::AlreadyExists);
    }
    Ok((name.to_owned(), TempFile::unique(parent)?))
}

/// Writes content into the prepared temporary file and publishes it atomically.
pub(crate) fn commit_write(
    (name, mut temporary): (String, TempFile),
    content: &[u8],
    mode: WriteMode,
) -> Result<u64, ServiceError> {
    temporary.write(content)?;
    temporary.finalize(&name, mode)?;
    Ok(crate::report::len64(content.len()))
}
