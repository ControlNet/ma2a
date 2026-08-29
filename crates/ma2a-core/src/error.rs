//! Closed errors carried by Phase 1 response envelopes.

use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    VersionMismatch,
    InvalidInput,
    Unauthorized,
    NotFound,
    Conflict,
    Expired,
    Rollback,
    Unavailable,
    Internal,
}

/// A deterministic member of the closed Phase 1 protocol error set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolError(ErrorKind);

impl ProtocolError {
    /// The peer uses an unsupported protocol version.
    pub const VERSION_MISMATCH: Self = Self(ErrorKind::VersionMismatch);
    /// The input is malformed, noncanonical, or outside a declared bound.
    pub const INVALID_INPUT: Self = Self(ErrorKind::InvalidInput);
    /// The authenticated endpoint is not permitted to perform the operation.
    pub const UNAUTHORIZED: Self = Self(ErrorKind::Unauthorized);
    /// The requested resource does not exist.
    pub const NOT_FOUND: Self = Self(ErrorKind::NotFound);
    /// The request conflicts with already accepted state.
    pub const CONFLICT: Self = Self(ErrorKind::Conflict);
    /// The requested capability or artifact has expired.
    pub const EXPIRED: Self = Self(ErrorKind::Expired);
    /// The request attempts to move accepted state backwards.
    pub const ROLLBACK: Self = Self(ErrorKind::Rollback);
    /// The requested operation is temporarily unavailable.
    pub const UNAVAILABLE: Self = Self(ErrorKind::Unavailable);
    /// The receiver encountered an internal failure.
    pub const INTERNAL: Self = Self(ErrorKind::Internal);

    /// Returns the stable snake-case wire name.
    pub const fn name(self) -> &'static str {
        match self.0 {
            ErrorKind::VersionMismatch => "version_mismatch",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Expired => "expired",
            ErrorKind::Rollback => "rollback",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Internal => "internal",
        }
    }

    pub(crate) const fn code(self) -> u8 {
        match self.0 {
            ErrorKind::VersionMismatch => 0,
            ErrorKind::InvalidInput => 1,
            ErrorKind::Unauthorized => 2,
            ErrorKind::NotFound => 3,
            ErrorKind::Conflict => 4,
            ErrorKind::Expired => 5,
            ErrorKind::Rollback => 6,
            ErrorKind::Unavailable => 7,
            ErrorKind::Internal => 8,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::VERSION_MISMATCH),
            1 => Some(Self::INVALID_INPUT),
            2 => Some(Self::UNAUTHORIZED),
            3 => Some(Self::NOT_FOUND),
            4 => Some(Self::CONFLICT),
            5 => Some(Self::EXPIRED),
            6 => Some(Self::ROLLBACK),
            7 => Some(Self::UNAVAILABLE),
            8 => Some(Self::INTERNAL),
            _ => None,
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.0 {
            ErrorKind::VersionMismatch => "protocol version mismatch",
            ErrorKind::InvalidInput => "invalid protocol input",
            ErrorKind::Unauthorized => "operation is unauthorized",
            ErrorKind::NotFound => "resource was not found",
            ErrorKind::Conflict => "request conflicts with accepted state",
            ErrorKind::Expired => "request or artifact has expired",
            ErrorKind::Rollback => "state rollback was rejected",
            ErrorKind::Unavailable => "operation is unavailable",
            ErrorKind::Internal => "internal protocol failure",
        };
        formatter.write_str(message)
    }
}

impl Error for ProtocolError {}
