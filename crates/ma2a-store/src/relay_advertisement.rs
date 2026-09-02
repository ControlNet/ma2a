use ma2a_core::{EndpointId, SignedPrivateRelayAdvertisementV1, SpaceAuthorizationView, SpaceId};

/// Validated private relay advertisement that alone can cross the persistence boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRelayAdvertisement {
    signed: SignedPrivateRelayAdvertisementV1,
    issued_at_ms: i64,
    expires_at_ms: i64,
}

impl ValidatedRelayAdvertisement {
    /// Parses and validates canonical bytes, exact Space authority, time, and provider signature.
    ///
    /// # Errors
    /// Returns [`RelayAdvertisementBoundaryError`] for every rejected trust boundary.
    pub fn parse(
        bytes: &[u8],
        authorization: &SpaceAuthorizationView,
        now_ms: u64,
    ) -> Result<Self, RelayAdvertisementBoundaryError> {
        let signed = SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(bytes)
            .map_err(|_| RelayAdvertisementBoundaryError::Malformed)?;
        let advertisement = signed.advertisement();
        if advertisement.space_id() != authorization.space_id() {
            return Err(RelayAdvertisementBoundaryError::WrongSpace);
        }
        if !authorization.allows_private_relay_provider(advertisement.provider_endpoint_id()) {
            return Err(RelayAdvertisementBoundaryError::UnauthorizedProvider);
        }
        if advertisement.issued_at_ms() > now_ms {
            return Err(RelayAdvertisementBoundaryError::FutureAdvertisement);
        }
        if advertisement.expires_at_ms() <= now_ms {
            return Err(RelayAdvertisementBoundaryError::ExpiredAdvertisement);
        }
        signed
            .verify_signature()
            .map_err(|_| RelayAdvertisementBoundaryError::InvalidSignature)?;
        let issued_at_ms = i64::try_from(advertisement.issued_at_ms())
            .map_err(|_| RelayAdvertisementBoundaryError::Malformed)?;
        let expires_at_ms = i64::try_from(advertisement.expires_at_ms())
            .map_err(|_| RelayAdvertisementBoundaryError::Malformed)?;
        Ok(Self {
            signed,
            issued_at_ms,
            expires_at_ms,
        })
    }

    /// Returns the accepted signed advertisement.
    pub const fn signed(&self) -> &SignedPrivateRelayAdvertisementV1 {
        &self.signed
    }

    pub(crate) const fn space_id(&self) -> SpaceId {
        self.signed.advertisement().space_id()
    }

    pub(crate) const fn provider_endpoint_id(&self) -> EndpointId {
        self.signed.advertisement().provider_endpoint_id()
    }

    pub(crate) const fn sequence(&self) -> u64 {
        self.signed.advertisement().sequence()
    }

    pub(crate) const fn issued_at_ms(&self) -> i64 {
        self.issued_at_ms
    }

    pub(crate) const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub(crate) fn advertisement_hash(&self) -> [u8; 32] {
        self.signed.advertisement_hash()
    }

    pub(crate) fn signed_advertisement(&self) -> &[u8] {
        self.signed.canonical_bytes()
    }
}

/// Rejection reason before private relay advertisement persistence is reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "network callers must map every closed advertisement trust-boundary rejection"
)]
pub enum RelayAdvertisementBoundaryError {
    /// Canonical advertisement bytes are malformed, oversized, or out of storage range.
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
}
