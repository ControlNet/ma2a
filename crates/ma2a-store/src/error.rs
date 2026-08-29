use std::{error::Error, fmt};

/// Actionable failures from the synchronous persistent-state boundary.
#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError {
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// `SQLite` rejected an operation.
    Sqlite(rusqlite::Error),
    /// A signed Space chain failed protocol validation.
    Manifest(ma2a_core::ManifestError),
    /// Space genesis construction or signing failed.
    Protocol(ma2a_core::ProtocolError),
    /// The database was created by a newer unsupported schema.
    FutureSchema {
        /// Version read from the database header.
        found: u32,
        /// Highest schema version this build supports.
        supported: u32,
    },
    /// Schema v1 is incomplete or inconsistent.
    SchemaMismatch {
        /// Stable description of the violated schema invariant.
        detail: &'static str,
    },
    /// A state or protected-key path has unsafe ownership or access.
    InsecurePermissions {
        /// Stable path class without disclosing the local path.
        target: &'static str,
        /// Access invariant that was violated.
        detail: &'static str,
    },
    /// The state directory is not a local absolute path.
    InvalidStateDirectory,
    /// An opaque protected-key reference is malformed.
    InvalidKeyReference,
    /// A protected key referenced by `SQLite` is unavailable.
    MissingProtectedKey {
        /// The private-key class required by the database record.
        kind: &'static str,
        /// Opaque non-secret reference stored in `SQLite`.
        reference: String,
    },
    /// A requested Space does not exist in this repository.
    SpaceNotFound,
    /// A public or imported Space has no local authority signing key.
    SpaceAuthorityUnavailable,
    /// A protected-key reference already has immutable material.
    ProtectedKeyAlreadyExists,
    /// The Windows permission helper failed closed.
    WindowsAcl {
        /// Whether DACL application or validation failed.
        operation: &'static str,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "persistent-state filesystem operation failed: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite persistent-state operation failed: {error}"),
            Self::Manifest(error) => write!(formatter, "signed Space chain rejected: {error}"),
            Self::Protocol(error) => write!(formatter, "Space protocol operation failed: {error}"),
            Self::FutureSchema { found, supported } => write!(
                formatter,
                "database schema version {found} is newer than supported version {supported}; upgrade MA2A before opening it"
            ),
            Self::SchemaMismatch { detail } => {
                write!(formatter, "database schema validation failed: {detail}")
            }
            Self::InsecurePermissions { target, detail } => write!(
                formatter,
                "refusing insecure {target} permissions: {detail}; restore current-user-only access"
            ),
            Self::InvalidStateDirectory => {
                formatter.write_str("state directory must be an absolute local path")
            }
            Self::InvalidKeyReference => formatter.write_str(
                "protected-key reference must contain only ASCII letters, digits, dot, dash, or underscore",
            ),
            Self::MissingProtectedKey { kind, reference } => write!(
                formatter,
                "database references missing or inaccessible {kind} protected key `{reference}`"
            ),
            Self::SpaceNotFound => formatter.write_str("Space was not found in this repository"),
            Self::SpaceAuthorityUnavailable => {
                formatter.write_str("Space has no local protected authority key")
            }
            Self::ProtectedKeyAlreadyExists => formatter.write_str(
                "protected-key material already exists; rotate by writing a new opaque reference",
            ),
            Self::WindowsAcl { operation } => write!(
                formatter,
                "Windows current-user-and-SYSTEM DACL {operation} failed; state remains unavailable"
            ),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::FutureSchema { .. }
            | Self::SchemaMismatch { .. }
            | Self::InsecurePermissions { .. }
            | Self::InvalidStateDirectory
            | Self::InvalidKeyReference
            | Self::MissingProtectedKey { .. }
            | Self::SpaceNotFound
            | Self::SpaceAuthorityUnavailable
            | Self::ProtectedKeyAlreadyExists
            | Self::WindowsAcl { .. } => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<ma2a_core::ManifestError> for StoreError {
    fn from(error: ma2a_core::ManifestError) -> Self {
        Self::Manifest(error)
    }
}

impl From<ma2a_core::ProtocolError> for StoreError {
    fn from(error: ma2a_core::ProtocolError) -> Self {
        Self::Protocol(error)
    }
}
