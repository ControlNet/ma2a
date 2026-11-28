use std::{error::Error, fmt};

use ma2a_net::{InvalidEndpointSecret, NetError, PrivateRelayServerError};
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
pub struct RuntimeError(RuntimeErrorKind, Option<u64>);

#[derive(Debug)]
pub(crate) enum RuntimeErrorKind {
    Store(StoreError),
    Network(NetError),
    PrivateRelay(ma2a_net::PrivateRelayServerError),
    InvalidEndpointKey(InvalidEndpointSecret),
    IdentityMismatch,
    KeyReferenceMismatch,
    Channel,
    Task(tokio::task::JoinError),
    Random,
    Clock,
    /// The clock is behind the newest issue time already published.
    ClockBehind,
    ObservationOverflow,
    AddressObservationUnavailable,
    Shutdown,
    Control,
    /// A targeted control sync found no shared eligible peer to dial.
    ControlNotDialed,
}

impl RuntimeError {
    pub(crate) const fn new(kind: RuntimeErrorKind) -> Self {
        Self(kind, None)
    }

    pub(crate) const fn after_commit(mut self, revision: u64) -> Self {
        self.1 = Some(revision);
        self
    }

    /// Returns the durable revision when the mutation committed before completion failed.
    pub const fn committed_revision(&self) -> Option<u64> {
        self.1
    }

    /// Background maintenance retries only recognized temporary operational failures.
    pub(crate) fn is_retryable_background(&self) -> bool {
        match &self.0 {
            RuntimeErrorKind::Store(error) => error.is_retryable_maintenance(),
            // A clock stepped back below the newest issue time catches up again.
            RuntimeErrorKind::AddressObservationUnavailable | RuntimeErrorKind::ClockBehind => true,
            RuntimeErrorKind::PrivateRelay(error) => error.is_retryable_configuration(),
            _ => false,
        }
    }

    /// Local address or relay publication is waiting for a publishable address or for the clock
    /// to catch up, so background maintenance will retry it later.
    pub(crate) const fn is_deferred_publication(&self) -> bool {
        matches!(
            self.0,
            RuntimeErrorKind::AddressObservationUnavailable | RuntimeErrorKind::ClockBehind
        )
    }

    /// A request this Runtime refuses by design: it ends that request and leaves the Runtime
    /// running.
    pub(crate) const fn is_request_refusal(&self) -> bool {
        matches!(
            self.0,
            RuntimeErrorKind::Store(
                StoreError::SpaceHistoryFull
                    | StoreError::SpaceMembersFull
                    | StoreError::SpaceOwnerCannotBeRemoved
                    | StoreError::DepartureReceiptsFull
            )
        )
    }

    /// The named Space, or the member a removal names, is not there.
    pub(crate) const fn is_space_not_found(&self) -> bool {
        matches!(self.0, RuntimeErrorKind::Store(StoreError::SpaceNotFound))
    }

    /// This Runtime holds the named Space but not its authority, so it cannot sign for it.
    pub(crate) const fn is_not_space_owner(&self) -> bool {
        matches!(
            self.0,
            RuntimeErrorKind::Store(StoreError::SpaceAuthorityUnavailable)
        )
    }

    /// The named Space already has as many members as its policy allows.
    pub(crate) const fn is_space_members_full(&self) -> bool {
        matches!(
            self.0,
            RuntimeErrorKind::Store(StoreError::SpaceMembersFull)
        )
    }

    /// A targeted control sync did not dial the requested peer at all.
    pub(crate) const fn is_control_not_dialed(&self) -> bool {
        matches!(self.0, RuntimeErrorKind::ControlNotDialed)
    }

    /// Pre-commit native TLS loading failed before any configuration commit.
    pub(crate) const fn is_pre_commit_relay_tls_refusal(&self) -> bool {
        matches!(
            (&self.0, self.1),
            (
                RuntimeErrorKind::PrivateRelay(PrivateRelayServerError::Tls(_)),
                None
            )
        )
    }

    /// Returns a stable non-secret error classification.
    pub const fn code(&self) -> RuntimeErrorCode {
        let kind = match &self.0 {
            RuntimeErrorKind::Store(StoreError::SpaceOwnerCannotBeRemoved) => {
                ErrorCodeKind::SpaceOwnerCannotBeRemoved
            }
            RuntimeErrorKind::Store(_) => ErrorCodeKind::Store,
            RuntimeErrorKind::Network(_) | RuntimeErrorKind::PrivateRelay(_) => {
                ErrorCodeKind::Network
            }
            RuntimeErrorKind::InvalidEndpointKey(_) => ErrorCodeKind::InvalidEndpointKey,
            RuntimeErrorKind::IdentityMismatch | RuntimeErrorKind::KeyReferenceMismatch => {
                ErrorCodeKind::IdentityMismatch
            }
            RuntimeErrorKind::Channel => ErrorCodeKind::Channel,
            RuntimeErrorKind::Task(_) => ErrorCodeKind::Task,
            RuntimeErrorKind::Random => ErrorCodeKind::Random,
            RuntimeErrorKind::Clock | RuntimeErrorKind::ClockBehind => ErrorCodeKind::Clock,
            RuntimeErrorKind::ObservationOverflow => ErrorCodeKind::ObservationOverflow,
            RuntimeErrorKind::Shutdown => ErrorCodeKind::Shutdown,
            RuntimeErrorKind::AddressObservationUnavailable
            | RuntimeErrorKind::Control
            | RuntimeErrorKind::ControlNotDialed => ErrorCodeKind::Control,
        };
        RuntimeErrorCode(kind)
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(revision) = self.1 {
            write!(
                formatter,
                "mutation committed at revision {revision}; completion failed: "
            )?;
        }
        match &self.0 {
            RuntimeErrorKind::Store(error) => write!(formatter, "Runtime storage failed: {error}"),
            RuntimeErrorKind::PrivateRelay(error) => error.fmt(formatter),
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
            RuntimeErrorKind::ClockBehind => {
                formatter.write_str("system clock is behind the newest published issue time")
            }
            RuntimeErrorKind::ObservationOverflow => {
                formatter.write_str("Runtime observation count exceeded u64")
            }
            RuntimeErrorKind::AddressObservationUnavailable => {
                formatter.write_str("Iroh has not supplied a publishable address observation yet")
            }
            RuntimeErrorKind::Shutdown => {
                formatter.write_str("Runtime shutdown did not join every owned task")
            }
            RuntimeErrorKind::Control => {
                formatter.write_str("Runtime control synchronization failed")
            }
            RuntimeErrorKind::ControlNotDialed => {
                formatter.write_str("Runtime found no eligible shared peer to synchronize")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0 {
            RuntimeErrorKind::Store(error) => Some(error),
            RuntimeErrorKind::Network(error) => Some(error),
            RuntimeErrorKind::PrivateRelay(error) => Some(error),
            RuntimeErrorKind::InvalidEndpointKey(error) => Some(error),
            RuntimeErrorKind::Task(error) => Some(error),
            RuntimeErrorKind::IdentityMismatch
            | RuntimeErrorKind::KeyReferenceMismatch
            | RuntimeErrorKind::Channel
            | RuntimeErrorKind::Random
            | RuntimeErrorKind::Clock
            | RuntimeErrorKind::ClockBehind
            | RuntimeErrorKind::ObservationOverflow
            | RuntimeErrorKind::AddressObservationUnavailable
            | RuntimeErrorKind::Shutdown
            | RuntimeErrorKind::Control
            | RuntimeErrorKind::ControlNotDialed => None,
        }
    }
}

#[cfg(test)]
mod background_policy_test;
mod conversions;
