use std::{error::Error, fmt};

use iroh::{Endpoint, EndpointAddr, SecretKey};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity,
    MAX_ADDRESS_RECORD_VALIDITY_MS, ProtocolError, SignedSpaceAddressRecordV1,
    SpaceAddressRecordV1, SpaceAuthorizationView,
};
use ma2a_store::{Repository, StoreError};

use crate::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator,
    ValidatedAddressRecord,
};

const ADDRESS_REFRESH_INTERVAL_MS: u64 = 300_000;

#[derive(Clone)]
enum PublisherSource {
    Live(Endpoint),
    Snapshot(EndpointAddr),
}

/// Endpoint-owned address publisher using live Iroh observations or a validated snapshot.
#[derive(Clone)]
pub struct AddressPublisher {
    secret: SecretKey,
    source: PublisherSource,
}

impl AddressPublisher {
    /// Creates a snapshot publisher after checking the address identity against the signer.
    ///
    /// # Errors
    /// Returns [`AddressPublisherError::IdentityMismatch`] before any record can be signed.
    pub fn new(
        secret: SecretKey,
        endpoint_addr: EndpointAddr,
    ) -> Result<Self, AddressPublisherError> {
        validate_observation(&secret, &endpoint_addr)?;
        Ok(Self {
            secret,
            source: PublisherSource::Snapshot(endpoint_addr),
        })
    }

    /// Creates a publisher that reads the current address from a running Iroh Endpoint.
    ///
    /// # Errors
    /// Returns [`AddressPublisherError`] when the Endpoint observation is not publishable.
    pub fn from_endpoint(endpoint: &Endpoint) -> Result<Self, AddressPublisherError> {
        validate_observation(endpoint.secret_key(), &endpoint.addr())?;
        Ok(Self {
            secret: endpoint.secret_key().clone(),
            source: PublisherSource::Live(endpoint.clone()),
        })
    }

    /// Publishes when address data changed or the five-minute refresh interval elapsed.
    ///
    /// # Errors
    /// Returns [`AddressPublisherError`] for invalid observations, clock rollback, storage, or validation failure.
    pub fn publish(
        &self,
        repository: &mut Repository,
        request: AddressPublishRequest<'_>,
    ) -> Result<Option<ValidatedAddressRecord>, AddressPublisherError> {
        let endpoint_addr = match &self.source {
            PublisherSource::Live(endpoint) => endpoint.addr(),
            PublisherSource::Snapshot(endpoint_addr) => endpoint_addr.clone(),
        };
        validate_observation(&self.secret, &endpoint_addr)?;
        let endpoint_data =
            AddressEndpointDataV1::new(endpoint_addr.addrs.iter().cloned().collect())
                .map_err(AddressPublisherError::Protocol)?;
        let endpoint_id = self.secret.public().into();
        let space_id = request.authorization.space_id();
        let current = repository
            .address_record(space_id, endpoint_id)
            .map_err(AddressPublisherError::Store)?;
        if let Some(current) = &current {
            let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(current.signed_record())
                .map_err(AddressPublisherError::Protocol)?;
            if request.now_ms < signed.record().issued_at_ms() {
                return Err(AddressPublisherError::ClockRollback);
            }
            let refresh_due =
                request.now_ms - signed.record().issued_at_ms() >= ADDRESS_REFRESH_INTERVAL_MS;
            if signed.record().endpoint_data() == &endpoint_data && !refresh_due {
                return Ok(None);
            }
        }
        let sequence = current.as_ref().map_or(Ok(0), |record| {
            record
                .sequence()
                .checked_add(1)
                .ok_or(AddressPublisherError::SequenceExhausted)
        })?;
        let expires_at_ms = request
            .now_ms
            .checked_add(MAX_ADDRESS_RECORD_VALIDITY_MS)
            .ok_or(AddressPublisherError::ClockOverflow)?;
        let scope = AddressRecordScope::new(space_id, endpoint_id);
        let validity = AddressRecordValidity::new(sequence, request.now_ms, expires_at_ms)
            .map_err(AddressPublisherError::Protocol)?;
        let signed = SpaceAddressRecordV1::new(scope, validity, endpoint_data)
            .sign(&self.secret)
            .map_err(AddressPublisherError::Protocol)?;
        let target = AddressRecordTarget::new(space_id, endpoint_id);
        AddressRecordValidator::validate_and_store(
            repository,
            signed.canonical_bytes(),
            target.validation(request.authorization, request.now_ms),
        )
        .map(Some)
        .map_err(AddressPublisherError::Validation)
    }
}

impl fmt::Debug for AddressPublisher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AddressPublisher { secret: [REDACTED] }")
    }
}

/// Current Space authorization and publish timestamp.
#[derive(Clone, Copy, Debug)]
pub struct AddressPublishRequest<'a> {
    authorization: &'a SpaceAuthorizationView,
    now_ms: u64,
}

impl<'a> AddressPublishRequest<'a> {
    /// Creates one deterministic publication decision.
    pub const fn new(authorization: &'a SpaceAuthorizationView, now_ms: u64) -> Self {
        Self {
            authorization,
            now_ms,
        }
    }
}

/// Address publication failure.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every fail-closed publisher state"
)]
pub enum AddressPublisherError {
    /// The observed Endpoint identity differs from the signer.
    IdentityMismatch,
    /// The observed transport data violates protocol bounds.
    Protocol(ProtocolError),
    /// Persistent address state could not be read.
    Store(StoreError),
    /// Ordered validation rejected the locally created record.
    Validation(AddressRecordValidationError),
    /// The injected clock moved behind the last persisted issue time.
    ClockRollback,
    /// Timestamp arithmetic overflowed.
    ClockOverflow,
    /// The monotonic sequence has no successor.
    SequenceExhausted,
}

impl fmt::Display for AddressPublisherError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityMismatch => {
                formatter.write_str("Endpoint address identity differs from signer")
            }
            Self::Protocol(error) => write!(formatter, "address observation is invalid: {error}"),
            Self::Store(error) => write!(formatter, "address state read failed: {error}"),
            Self::Validation(error) => {
                write!(formatter, "published address record was rejected: {error}")
            }
            Self::ClockRollback => formatter.write_str("publisher clock moved backwards"),
            Self::ClockOverflow => formatter.write_str("publisher clock overflowed"),
            Self::SequenceExhausted => formatter.write_str("address record sequence is exhausted"),
        }
    }
}

impl Error for AddressPublisherError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::IdentityMismatch
            | Self::ClockRollback
            | Self::ClockOverflow
            | Self::SequenceExhausted => None,
        }
    }
}

fn validate_observation(
    secret: &SecretKey,
    endpoint_addr: &EndpointAddr,
) -> Result<(), AddressPublisherError> {
    if endpoint_addr.id != secret.public() {
        return Err(AddressPublisherError::IdentityMismatch);
    }
    AddressEndpointDataV1::new(endpoint_addr.addrs.iter().cloned().collect())
        .map(|_| ())
        .map_err(AddressPublisherError::Protocol)
}
