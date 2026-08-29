use std::{error::Error, fmt};

use ma2a_core::{EndpointId, SignedSpaceAddressRecordV1, SpaceAuthorizationView, SpaceId};
use ma2a_store::{AddressAdvance, AddressRecordOutcome, Repository, StoreError};

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
        }
    }
}

/// Inputs that must remain consistent during one validation decision.
#[derive(Clone, Copy, Debug)]
pub struct AddressValidationContext<'a> {
    target: AddressRecordTarget,
    authorization: &'a SpaceAuthorizationView,
    now_ms: u64,
}

/// A canonical, authorized, current, signed, and persistently accepted address record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedAddressRecord(SignedSpaceAddressRecordV1);

impl ValidatedAddressRecord {
    /// Returns the accepted signed record.
    pub const fn record(&self) -> &SignedSpaceAddressRecordV1 {
        &self.0
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
    /// Validates and persistently accepts one exact target record.
    ///
    /// # Errors
    /// Returns the first failure in canonical, identity, membership, clock, signature, and sequence order.
    pub fn validate_and_store(
        repository: &mut Repository,
        bytes: &[u8],
        context: AddressValidationContext<'_>,
    ) -> Result<ValidatedAddressRecord, AddressRecordValidationError> {
        let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(bytes)
            .map_err(|_| AddressRecordValidationError::InvalidEncoding)?;
        let record = signed.record();
        if record.endpoint_id() != context.target.endpoint_id {
            return Err(AddressRecordValidationError::WrongEndpoint);
        }
        if context.authorization.space_id() != context.target.space_id
            || record.space_id() != context.target.space_id
        {
            return Err(AddressRecordValidationError::WrongSpace);
        }
        if !context
            .authorization
            .contains_member(context.target.endpoint_id)
        {
            return Err(AddressRecordValidationError::UnauthorizedMember);
        }
        if record.issued_at_ms() > context.now_ms {
            return Err(AddressRecordValidationError::FutureRecord);
        }
        if record.expires_at_ms() <= context.now_ms {
            return Err(AddressRecordValidationError::ExpiredRecord);
        }
        signed
            .verify_signature()
            .map_err(|_| AddressRecordValidationError::InvalidSignature)?;
        let issued_at_ms = i64::try_from(record.issued_at_ms())
            .map_err(|_| AddressRecordValidationError::InvalidEncoding)?;
        let expires_at_ms = i64::try_from(record.expires_at_ms())
            .map_err(|_| AddressRecordValidationError::InvalidEncoding)?;
        let outcome = repository
            .advance_address(&AddressAdvance {
                space_id: record.space_id(),
                endpoint_id: record.endpoint_id(),
                sequence: record.sequence(),
                issued_at_ms,
                expires_at_ms,
                record_hash: signed.record_hash(),
                signed_record: signed.canonical_bytes().to_vec(),
            })
            .map_err(AddressRecordValidationError::Store)?;
        match outcome {
            AddressRecordOutcome::Advanced { .. } | AddressRecordOutcome::Idempotent { .. } => {
                Ok(ValidatedAddressRecord(signed))
            }
            AddressRecordOutcome::Rollback { .. } => Err(AddressRecordValidationError::Rollback),
            AddressRecordOutcome::Fork { .. } => Err(AddressRecordValidationError::Fork),
        }
    }
}
