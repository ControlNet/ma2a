use std::{error::Error, fmt};

use ma2a_core::{
    Capability, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SignedPrivateRelayAdvertisementV1, SpaceAuthorizationView,
};
use ma2a_store::{
    RelayAdvertisementBoundaryError, RelayAdvertisementOutcome, Repository, StoreError,
    ValidatedRelayAdvertisement,
};

use crate::{EndpointSecret, PrivateRelayProviderConfig};

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

/// Current authorization and validity inputs for one publication refresh.
#[derive(Clone, Copy, Debug)]
pub struct AdvertisementPublicationRequest<'a> {
    authorizations: &'a [SpaceAuthorizationView],
    issued_at_ms: u64,
    expires_at_ms: u64,
}

impl<'a> AdvertisementPublicationRequest<'a> {
    /// Creates one coherent publication request.
    pub const fn new(
        authorizations: &'a [SpaceAuthorizationView],
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            authorizations,
            issued_at_ms,
            expires_at_ms,
        }
    }
}

/// Emits one independently signed advertisement per configured served Space.
#[derive(Clone, Debug)]
pub struct PrivateRelayAdvertisementPublisher {
    secret: EndpointSecret,
    config: PrivateRelayProviderConfig,
}

impl PrivateRelayAdvertisementPublisher {
    /// Binds advertisements to the existing Runtime Endpoint identity.
    pub const fn new(secret: EndpointSecret, config: PrivateRelayProviderConfig) -> Self {
        Self { secret, config }
    }

    /// Persists one sequence and signs advertisements only for effective served Spaces.
    ///
    /// # Errors
    /// Returns [`PrivateRelayAdvertisementPublishError`] when persistence or signing fails.
    pub fn advertisements(
        &self,
        repository: &mut Repository,
        request: AdvertisementPublicationRequest<'_>,
    ) -> Result<Vec<SignedPrivateRelayAdvertisementV1>, PrivateRelayAdvertisementPublishError> {
        let provider_endpoint_id = self.secret.endpoint_id();
        let effective_spaces = request
            .authorizations
            .iter()
            .filter(|authorization| {
                self.config
                    .served_spaces()
                    .contains(&authorization.space_id())
                    && authorization
                        .allows(provider_endpoint_id, Capability::PRIVATE_RELAY_PROVIDER)
            })
            .map(SpaceAuthorizationView::space_id)
            .collect::<Vec<_>>();
        if effective_spaces.is_empty() {
            return Ok(Vec::new());
        }
        let sequence = repository.reserve_private_relay_sequence()?;
        let validity = PrivateRelayAdvertisementValidity::new(
            sequence,
            request.issued_at_ms,
            request.expires_at_ms,
        )?;
        effective_spaces
            .into_iter()
            .map(|space_id| {
                PrivateRelayAdvertisementV1::new(
                    PrivateRelayAdvertisementScope::new(space_id, provider_endpoint_id),
                    self.config.public_relay_url().clone(),
                    validity,
                )?
                .sign(self.secret.iroh_secret())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(PrivateRelayAdvertisementPublishError::Protocol)
    }
}

/// Private relay advertisement publication failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum PrivateRelayAdvertisementPublishError {
    /// Sequence persistence failed before signing.
    Store(StoreError),
    /// Advertisement validity or signing failed.
    Protocol(ma2a_core::ProtocolError),
}

impl fmt::Display for PrivateRelayAdvertisementPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "private relay publication failed: {error}"),
            Self::Protocol(error) => write!(formatter, "private relay signing failed: {error}"),
        }
    }
}

impl Error for PrivateRelayAdvertisementPublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Protocol(error) => Some(error),
        }
    }
}

impl From<StoreError> for PrivateRelayAdvertisementPublishError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ma2a_core::ProtocolError> for PrivateRelayAdvertisementPublishError {
    fn from(error: ma2a_core::ProtocolError) -> Self {
        Self::Protocol(error)
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
