use std::{error::Error, fmt};

use ma2a_core::{SignedPrivateRelayAdvertisementV1, SpaceAuthorizationView};
use ma2a_store::{
    RelayAdvertisementBoundaryError, RelayAdvertisementOutcome, Repository, StoreError,
    ValidatedRelayAdvertisement,
};

/// Consistent authorization and clock inputs for one advertisement validation decision.
#[derive(Clone, Copy, Debug)]
pub struct AdvertisementValidationContext<'a> {
    authorization: &'a SpaceAuthorizationView,
    now_ms: u64,
}

impl<'a> AdvertisementValidationContext<'a> {
    /// Creates exact-Space validation inputs.
    pub const fn new(authorization: &'a SpaceAuthorizationView, now_ms: u64) -> Self {
        Self {
            authorization,
            now_ms,
        }
    }
}

/// Canonical, authorized, current, signed, and persistently accepted advertisement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedPrivateRelayAdvertisement {
    signed: SignedPrivateRelayAdvertisementV1,
}

impl ValidatedPrivateRelayAdvertisement {
    /// Returns the accepted signed advertisement.
    pub const fn signed(&self) -> &SignedPrivateRelayAdvertisementV1 {
        &self.signed
    }
}

/// Validates and atomically advances private relay advertisement state.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct PrivateRelayAdvertisementValidator;

impl PrivateRelayAdvertisementValidator {
    /// Validates one advertisement without mutating persistent state.
    ///
    /// # Errors
    /// Returns a trust-boundary rejection for malformed or unauthorized input.
    pub fn validate(
        bytes: &[u8],
        context: AdvertisementValidationContext<'_>,
    ) -> Result<ValidatedRelayAdvertisement, PrivateRelayAdvertisementValidationError> {
        ValidatedRelayAdvertisement::parse(bytes, context.authorization, context.now_ms)
            .map_err(Into::into)
    }

    /// Validates exact-Space provider authority before persistent high-water advancement.
    ///
    /// # Errors
    /// Returns [`PrivateRelayAdvertisementValidationError`] for every rejected boundary.
    pub fn validate_and_store(
        repository: &mut Repository,
        bytes: &[u8],
        context: AdvertisementValidationContext<'_>,
    ) -> Result<ValidatedPrivateRelayAdvertisement, PrivateRelayAdvertisementValidationError> {
        let advance = Self::validate(bytes, context)?;
        let signed = advance.signed().clone();
        match repository.advance_private_relay_advertisement(&advance)? {
            RelayAdvertisementOutcome::Advanced { .. }
            | RelayAdvertisementOutcome::Idempotent { .. } => {
                Ok(ValidatedPrivateRelayAdvertisement { signed })
            }
            RelayAdvertisementOutcome::Rollback { current_sequence } => {
                Err(PrivateRelayAdvertisementValidationError::Rollback { current_sequence })
            }
            RelayAdvertisementOutcome::Fork { current_sequence } => {
                Err(PrivateRelayAdvertisementValidationError::Fork { current_sequence })
            }
        }
    }
}

/// Private relay advertisement validation failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum PrivateRelayAdvertisementValidationError {
    /// Canonical advertisement bytes are malformed or oversized.
    Malformed,
    /// Advertisement is not scoped to the authorization view's exact Space.
    WrongSpace,
    /// Provider is not a current capable member of the advertised Space.
    UnauthorizedProvider,
    /// Advertisement issue time is in the future.
    FutureAdvertisement,
    /// Advertisement has expired.
    ExpiredAdvertisement,
    /// Provider signature is invalid.
    InvalidSignature,
    /// Sequence attempts to roll accepted state backward.
    Rollback {
        /// Highest accepted sequence.
        current_sequence: u64,
    },
    /// Different signed bytes reuse the accepted sequence.
    Fork {
        /// Highest accepted sequence.
        current_sequence: u64,
    },
    /// Persistent state failed.
    Store(StoreError),
}

impl fmt::Display for PrivateRelayAdvertisementValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("private relay advertisement is malformed"),
            Self::WrongSpace => {
                formatter.write_str("private relay advertisement has the wrong Space")
            }
            Self::UnauthorizedProvider => {
                formatter.write_str("private relay provider is not authorized")
            }
            Self::FutureAdvertisement => {
                formatter.write_str("private relay advertisement is not yet valid")
            }
            Self::ExpiredAdvertisement => {
                formatter.write_str("private relay advertisement is expired")
            }
            Self::InvalidSignature => {
                formatter.write_str("private relay advertisement signature is invalid")
            }
            Self::Rollback { current_sequence } => write!(
                formatter,
                "private relay advertisement rolls back sequence {current_sequence}"
            ),
            Self::Fork { current_sequence } => write!(
                formatter,
                "private relay advertisement forks sequence {current_sequence}"
            ),
            Self::Store(error) => write!(
                formatter,
                "private relay advertisement persistence failed: {error}"
            ),
        }
    }
}

impl Error for PrivateRelayAdvertisementValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Malformed
            | Self::WrongSpace
            | Self::UnauthorizedProvider
            | Self::FutureAdvertisement
            | Self::ExpiredAdvertisement
            | Self::InvalidSignature
            | Self::Rollback { .. }
            | Self::Fork { .. } => None,
        }
    }
}

impl From<StoreError> for PrivateRelayAdvertisementValidationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RelayAdvertisementBoundaryError> for PrivateRelayAdvertisementValidationError {
    fn from(error: RelayAdvertisementBoundaryError) -> Self {
        match error {
            RelayAdvertisementBoundaryError::Malformed => Self::Malformed,
            RelayAdvertisementBoundaryError::WrongSpace => Self::WrongSpace,
            RelayAdvertisementBoundaryError::UnauthorizedProvider => Self::UnauthorizedProvider,
            RelayAdvertisementBoundaryError::FutureAdvertisement => Self::FutureAdvertisement,
            RelayAdvertisementBoundaryError::ExpiredAdvertisement => Self::ExpiredAdvertisement,
            RelayAdvertisementBoundaryError::InvalidSignature => Self::InvalidSignature,
        }
    }
}
