use std::{error::Error, fmt};

use ma2a_net::{InvalidEndpointSecret, NetError};
use ma2a_store::StoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorCodeKind {
    Store,
    Network,
    InvalidEndpointKey,
    IdentityMismatch,
    Channel,
    Task,
    Random,
    Clock,
    ObservationOverflow,
    Shutdown,
}

/// Stable classification for Runtime failures without secret-bearing details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeErrorCode(ErrorCodeKind);

impl RuntimeErrorCode {
    /// Persistent storage or protected-filesystem failure.
    pub const STORE: Self = Self(ErrorCodeKind::Store);
    /// Iroh Endpoint lifecycle failure.
    pub const NETWORK: Self = Self(ErrorCodeKind::Network);
    /// Protected Endpoint key bytes had an invalid length.
    pub const INVALID_ENDPOINT_KEY: Self = Self(ErrorCodeKind::InvalidEndpointKey);
    /// Persisted public identity did not match the protected private key.
    pub const IDENTITY_MISMATCH: Self = Self(ErrorCodeKind::IdentityMismatch);
}

/// Runtime lifecycle failure with redacted secret handling.
#[derive(Debug)]
pub struct RuntimeError(RuntimeErrorKind);

#[derive(Debug)]
pub(crate) enum RuntimeErrorKind {
    Store(StoreError),
    Network(NetError),
    InvalidEndpointKey(InvalidEndpointSecret),
    IdentityMismatch,
    KeyReferenceMismatch,
    Channel,
    Task(tokio::task::JoinError),
    Random,
    Clock,
    ObservationOverflow,
    Shutdown,
}

impl RuntimeError {
    pub(crate) const fn new(kind: RuntimeErrorKind) -> Self {
        Self(kind)
    }

    /// Returns a stable non-secret error classification.
    pub const fn code(&self) -> RuntimeErrorCode {
        let kind = match &self.0 {
            RuntimeErrorKind::Store(_) => ErrorCodeKind::Store,
            RuntimeErrorKind::Network(_) => ErrorCodeKind::Network,
            RuntimeErrorKind::InvalidEndpointKey(_) => ErrorCodeKind::InvalidEndpointKey,
            RuntimeErrorKind::IdentityMismatch | RuntimeErrorKind::KeyReferenceMismatch => {
                ErrorCodeKind::IdentityMismatch
            }
            RuntimeErrorKind::Channel => ErrorCodeKind::Channel,
            RuntimeErrorKind::Task(_) => ErrorCodeKind::Task,
            RuntimeErrorKind::Random => ErrorCodeKind::Random,
            RuntimeErrorKind::Clock => ErrorCodeKind::Clock,
            RuntimeErrorKind::ObservationOverflow => ErrorCodeKind::ObservationOverflow,
            RuntimeErrorKind::Shutdown => ErrorCodeKind::Shutdown,
        };
        RuntimeErrorCode(kind)
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            RuntimeErrorKind::Store(error) => write!(formatter, "Runtime storage failed: {error}"),
            RuntimeErrorKind::Network(error) => {
                write!(formatter, "Runtime network failed: {error}")
            }
            RuntimeErrorKind::InvalidEndpointKey(error) => error.fmt(formatter),
            RuntimeErrorKind::IdentityMismatch => formatter
                .write_str("persisted Endpoint identity does not match its protected private key"),
            RuntimeErrorKind::KeyReferenceMismatch => formatter.write_str(
                "persisted Endpoint key reference does not match the Runtime identity slot",
            ),
            RuntimeErrorKind::Channel => formatter.write_str("Runtime actor channel closed"),
            RuntimeErrorKind::Task(error) => {
                write!(formatter, "Runtime owned task failed: {error}")
            }
            RuntimeErrorKind::Random => {
                formatter.write_str("operating-system random source failed")
            }
            RuntimeErrorKind::Clock => formatter.write_str("system clock precedes Unix epoch"),
            RuntimeErrorKind::ObservationOverflow => {
                formatter.write_str("Runtime observation count exceeded u64")
            }
            RuntimeErrorKind::Shutdown => {
                formatter.write_str("Runtime shutdown did not join every owned task")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0 {
            RuntimeErrorKind::Store(error) => Some(error),
            RuntimeErrorKind::Network(error) => Some(error),
            RuntimeErrorKind::InvalidEndpointKey(error) => Some(error),
            RuntimeErrorKind::Task(error) => Some(error),
            RuntimeErrorKind::IdentityMismatch
            | RuntimeErrorKind::KeyReferenceMismatch
            | RuntimeErrorKind::Channel
            | RuntimeErrorKind::Random
            | RuntimeErrorKind::Clock
            | RuntimeErrorKind::ObservationOverflow
            | RuntimeErrorKind::Shutdown => None,
        }
    }
}

impl From<StoreError> for RuntimeError {
    fn from(error: StoreError) -> Self {
        Self(RuntimeErrorKind::Store(error))
    }
}

impl From<NetError> for RuntimeError {
    fn from(error: NetError) -> Self {
        Self(RuntimeErrorKind::Network(error))
    }
}

impl From<InvalidEndpointSecret> for RuntimeError {
    fn from(error: InvalidEndpointSecret) -> Self {
        Self(RuntimeErrorKind::InvalidEndpointKey(error))
    }
}

impl From<tokio::task::JoinError> for RuntimeError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self(RuntimeErrorKind::Task(error))
    }
}
