use std::{error::Error, fmt};

use ma2a_net::{InvalidEndpointSecret, NetError};
use ma2a_store::StoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorCodeKind {
    SpaceOwnerCannotBeRemoved,
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
    Control,
}

/// Stable classification for Runtime failures without secret-bearing details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeErrorCode(ErrorCodeKind);

impl RuntimeErrorCode {
    /// Phase 1 forbids removing the genesis owner from a locally owned Space.
    pub const SPACE_OWNER_CANNOT_BE_REMOVED: Self = Self(ErrorCodeKind::SpaceOwnerCannotBeRemoved);
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
    AddressObservationUnavailable,
    Shutdown,
    Control,
}

impl RuntimeError {
    pub(crate) const fn new(kind: RuntimeErrorKind) -> Self {
        Self(kind)
    }

    /// Background maintenance retries only recognized temporary operational failures.
    pub(crate) fn is_retryable_background(&self) -> bool {
        match &self.0 {
            RuntimeErrorKind::Store(error) => error.is_retryable_maintenance(),
            RuntimeErrorKind::AddressObservationUnavailable => true,
            _ => false,
        }
    }

    /// Returns a stable non-secret error classification.
    pub const fn code(&self) -> RuntimeErrorCode {
        let kind = match &self.0 {
            RuntimeErrorKind::Store(StoreError::SpaceOwnerCannotBeRemoved) => {
                ErrorCodeKind::SpaceOwnerCannotBeRemoved
            }
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
            RuntimeErrorKind::AddressObservationUnavailable | RuntimeErrorKind::Control => {
                ErrorCodeKind::Control
            }
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
            RuntimeErrorKind::AddressObservationUnavailable => {
                formatter.write_str("Iroh has not supplied a live address observation yet")
            }
            RuntimeErrorKind::Shutdown => {
                formatter.write_str("Runtime shutdown did not join every owned task")
            }
            RuntimeErrorKind::Control => {
                formatter.write_str("Runtime control synchronization failed")
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
            | RuntimeErrorKind::AddressObservationUnavailable
            | RuntimeErrorKind::Shutdown
            | RuntimeErrorKind::Control => None,
        }
    }
}

impl From<StoreError> for RuntimeError {
    fn from(error: StoreError) -> Self {
        Self(RuntimeErrorKind::Store(error))
    }
}

impl From<ma2a_net::AddressPublisherError> for RuntimeError {
    fn from(error: ma2a_net::AddressPublisherError) -> Self {
        match error {
            ma2a_net::AddressPublisherError::Store(error)
            | ma2a_net::AddressPublisherError::Validation(
                ma2a_net::AddressRecordValidationError::Store(error),
            ) => Self::from(error),
            ma2a_net::AddressPublisherError::ObservationUnavailable => {
                Self::new(RuntimeErrorKind::AddressObservationUnavailable)
            }
            _ => Self::new(RuntimeErrorKind::Control),
        }
    }
}

impl From<ma2a_net::PrivateRelayAdvertisementPublishError> for RuntimeError {
    fn from(error: ma2a_net::PrivateRelayAdvertisementPublishError) -> Self {
        match error {
            ma2a_net::PrivateRelayAdvertisementPublishError::Store(error) => Self::from(error),
            _ => Self::new(RuntimeErrorKind::Control),
        }
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

#[cfg(test)]
mod background_policy_test {
    use super::RuntimeError;

    #[test]
    fn only_unavailable_live_address_observation_is_retryable() {
        assert!(
            RuntimeError::from(ma2a_net::AddressPublisherError::ObservationUnavailable)
                .is_retryable_background()
        );
        assert!(
            !RuntimeError::from(ma2a_net::AddressPublisherError::ObservationPoisoned)
                .is_retryable_background()
        );
    }
}
