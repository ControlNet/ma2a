//! Mapping from operating-system failures to the closed service vocabulary.

use std::io::{self, ErrorKind};

use ma2a_core::service::ServiceError;

/// Maps one I/O failure without disclosing its text, paths, or raw OS code.
pub(crate) fn service_error(error: &io::Error) -> ServiceError {
    match error.kind() {
        ErrorKind::NotFound => ServiceError::NotFound,
        ErrorKind::AlreadyExists => ServiceError::AlreadyExists,
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
            ServiceError::PermissionDenied
        }
        ErrorKind::NotADirectory => ServiceError::NotADirectory,
        ErrorKind::IsADirectory => ServiceError::IsADirectory,
        ErrorKind::DirectoryNotEmpty => ServiceError::NotEmpty,
        ErrorKind::InvalidFilename | ErrorKind::InvalidInput => ServiceError::InvalidPath,
        ErrorKind::FileTooLarge | ErrorKind::StorageFull | ErrorKind::QuotaExceeded => {
            ServiceError::TooLarge
        }
        _ => ServiceError::Io,
    }
}

/// Converts one I/O result into the service vocabulary.
pub(crate) fn map<T>(result: io::Result<T>) -> Result<T, ServiceError> {
    result.map_err(|error| service_error(&error))
}
