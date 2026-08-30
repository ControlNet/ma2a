use std::{error::Error, fmt};

use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};
use ma2a_store::{
    AddressRecordBoundaryError, AddressRecordOutcome,
    AddressRecordTarget as StoreAddressRecordTarget, AddressRecordValidation, Repository,
    StoreError, ValidatedAddressRecord,
};

use crate::{AddressMetrics, AddressPersistenceOutcome, AddressValidationOutcome};

/// Exact Space and Endpoint requested by a private address lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRecordTarget {
    space_id: SpaceId,
    endpoint_id: EndpointId,
}

impl AddressRecordTarget {
    /// Creates an exact private lookup target.
    pub const fn new(space_id: SpaceId, endpoint_id: EndpointId) -> Self {
        Self {
            space_id,
            endpoint_id,
        }
    }

    /// Binds this target to a current Space authorization view and clock observation.
    pub const fn validation(
        self,
        authorization: &SpaceAuthorizationView,
        now_ms: u64,
    ) -> AddressValidationContext<'_> {
        AddressValidationContext {
            target: self,
            authorization,
            now_ms,
            metrics: None,
        }
    }
}

/// Inputs that must remain consistent during one validation decision.
#[derive(Clone, Copy, Debug)]
pub struct AddressValidationContext<'a> {
    target: AddressRecordTarget,
    authorization: &'a SpaceAuthorizationView,
    now_ms: u64,
    metrics: Option<&'a AddressMetrics>,
}

impl<'a> AddressValidationContext<'a> {
    /// Attaches privacy-safe outcome counters to this validation decision.
    #[must_use]
    pub const fn with_metrics(mut self, metrics: &'a AddressMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }
}

/// Ordered validation failure for a signed Space address record.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must distinguish every fail-closed address rejection"
)]
pub enum AddressRecordValidationError {
    /// Canonical decoding or structural bounds failed.
    InvalidEncoding,
    /// The signed Space does not match the requested Space.
    WrongSpace,
    /// The signed Endpoint does not match the requested Endpoint.
    WrongEndpoint,
    /// The Endpoint is not a current member of this exact Space.
    UnauthorizedMember,
    /// The record issue time is in the future.
    FutureRecord,
    /// The record is expired at the validation clock.
    ExpiredRecord,
    /// The Endpoint signature is invalid.
    InvalidSignature,
    /// Persistent state rejected a lower sequence.
    Rollback,
    /// Persistent state rejected different bytes at the same sequence.
    Fork,
    /// Persistent state could not be read or committed.
    Store(StoreError),
}

impl fmt::Display for AddressRecordValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEncoding => formatter.write_str("invalid canonical address record"),
            Self::WrongSpace => formatter.write_str("address record has the wrong Space"),
            Self::WrongEndpoint => formatter.write_str("address record has the wrong Endpoint"),
            Self::UnauthorizedMember => {
                formatter.write_str("Endpoint is not a current Space member")
            }
            Self::FutureRecord => formatter.write_str("address record issue time is in the future"),
            Self::ExpiredRecord => formatter.write_str("address record is expired"),
            Self::InvalidSignature => formatter.write_str("address record signature is invalid"),
            Self::Rollback => formatter.write_str("address record sequence would roll state back"),
            Self::Fork => {
                formatter.write_str("address record sequence conflicts with accepted bytes")
            }
            Self::Store(error) => write!(formatter, "address record persistence failed: {error}"),
        }
    }
}

impl Error for AddressRecordValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::InvalidEncoding
            | Self::WrongSpace
            | Self::WrongEndpoint
            | Self::UnauthorizedMember
            | Self::FutureRecord
            | Self::ExpiredRecord
            | Self::InvalidSignature
            | Self::Rollback
            | Self::Fork => None,
        }
    }
}

/// Stateless ordered validator for untrusted address record bytes.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AddressRecordValidator;

impl AddressRecordValidator {
    /// Validates one exact target record without mutating persistent state.
    ///
    /// # Errors
    /// Returns the first canonical, identity, membership, clock, or signature failure.
    pub fn validate(
        bytes: &[u8],
        context: AddressValidationContext<'_>,
    ) -> Result<ValidatedAddressRecord, AddressRecordValidationError> {
        let default_metrics = AddressMetrics::default();
        let metrics = context.metrics.unwrap_or(&default_metrics);
        ValidatedAddressRecord::parse(
            bytes,
            AddressRecordValidation::new(
                StoreAddressRecordTarget::new(context.target.space_id, context.target.endpoint_id),
                context.authorization,
                context.now_ms,
            ),
        )
        .map_err(|error| {
            let (outcome, error) = match error {
                AddressRecordBoundaryError::InvalidEncoding => (
                    AddressValidationOutcome::InvalidEncoding,
                    AddressRecordValidationError::InvalidEncoding,
                ),
                AddressRecordBoundaryError::WrongSpace => (
                    AddressValidationOutcome::WrongSpace,
                    AddressRecordValidationError::WrongSpace,
                ),
                AddressRecordBoundaryError::WrongEndpoint => (
                    AddressValidationOutcome::WrongEndpoint,
                    AddressRecordValidationError::WrongEndpoint,
                ),
                AddressRecordBoundaryError::UnauthorizedMember => (
                    AddressValidationOutcome::UnauthorizedMember,
                    AddressRecordValidationError::UnauthorizedMember,
                ),
                AddressRecordBoundaryError::FutureRecord => (
                    AddressValidationOutcome::FutureRecord,
                    AddressRecordValidationError::FutureRecord,
                ),
                AddressRecordBoundaryError::ExpiredRecord => (
                    AddressValidationOutcome::ExpiredRecord,
                    AddressRecordValidationError::ExpiredRecord,
                ),
                AddressRecordBoundaryError::InvalidSignature => (
                    AddressValidationOutcome::InvalidSignature,
                    AddressRecordValidationError::InvalidSignature,
                ),
            };
            metrics.record_validation(outcome);
            error
        })
    }

    /// Validates and persistently accepts one exact target record.
    ///
    /// # Errors
    /// Returns the first failure in canonical, identity, membership, clock, signature, and sequence order.
    pub fn validate_and_store(
        repository: &mut Repository,
        bytes: &[u8],
        context: AddressValidationContext<'_>,
    ) -> Result<ValidatedAddressRecord, AddressRecordValidationError> {
        let validated = Self::validate(bytes, context)?;
        let default_metrics = AddressMetrics::default();
        let metrics = context.metrics.unwrap_or(&default_metrics);
        let outcome = repository
            .advance_validated_address(&validated)
            .map_err(|error| {
                metrics.record_validation(AddressValidationOutcome::StoreError);
                AddressRecordValidationError::Store(error)
            })?;
        match outcome {
            AddressRecordOutcome::Advanced { .. } => {
                metrics.record_persistence(AddressPersistenceOutcome::Advanced);
                metrics.record_validation(AddressValidationOutcome::Accepted);
                Ok(validated)
            }
            AddressRecordOutcome::Idempotent { .. } => {
                metrics.record_persistence(AddressPersistenceOutcome::Idempotent);
                metrics.record_validation(AddressValidationOutcome::Accepted);
                Ok(validated)
            }
            AddressRecordOutcome::Rollback { .. } => {
                metrics.record_persistence(AddressPersistenceOutcome::Rollback);
                metrics.record_validation(AddressValidationOutcome::Rollback);
                Err(AddressRecordValidationError::Rollback)
            }
            AddressRecordOutcome::Fork { .. } => {
                metrics.record_persistence(AddressPersistenceOutcome::Fork);
                metrics.record_validation(AddressValidationOutcome::Fork);
                Err(AddressRecordValidationError::Fork)
            }
        }
    }
}
