use std::{error::Error, fmt};

use ma2a_core::{
    PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
    SignedPrivateRelayAdvertisementV1, SpaceAuthorizationView,
};
use ma2a_store::{Repository, StoreError};

use crate::{EndpointSecret, PrivateRelayProviderConfig};

mod validation;
pub use validation::{
    AdvertisementValidationContext, PrivateRelayAdvertisementValidationError,
    PrivateRelayAdvertisementValidator, ValidatedPrivateRelayAdvertisement,
};

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

    /// Returns the exact provider configuration used for signing.
    pub const fn config(&self) -> &PrivateRelayProviderConfig {
        &self.config
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
                    && authorization.allows_private_relay_provider(provider_endpoint_id)
            })
            .map(SpaceAuthorizationView::space_id)
            .collect::<Vec<_>>();
        if effective_spaces.is_empty() {
            return Ok(Vec::new());
        }
        // Check persisted issue times before reserving a sequence, even if the stored bytes no
        // longer decode, so a clock step back never advances or backdates an advertisement.
        for &space_id in &effective_spaces {
            if repository
                .relay_advertisement(space_id, provider_endpoint_id)?
                .is_some_and(|current| {
                    u64::try_from(current.issued_at_ms())
                        .is_ok_and(|issued| request.issued_at_ms < issued)
                })
            {
                return Err(PrivateRelayAdvertisementPublishError::ClockRollback);
            }
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
    /// The injected clock moved behind a persisted advertisement's issue time.
    ClockRollback,
}

impl fmt::Display for PrivateRelayAdvertisementPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "private relay publication failed: {error}"),
            Self::Protocol(error) => write!(formatter, "private relay signing failed: {error}"),
            Self::ClockRollback => formatter.write_str("publisher clock moved backwards"),
        }
    }
}

impl Error for PrivateRelayAdvertisementPublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::ClockRollback => None,
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
