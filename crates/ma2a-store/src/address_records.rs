use ma2a_core::{EndpointId, SignedSpaceAddressRecordV1, SpaceAuthorizationView, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::{
    Repository, StoreError, repository::increment_revision, repository_models::AddressAdvance,
};

/// A canonical, authorized, current, signer-bound address record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedAddressRecord {
    signed: SignedSpaceAddressRecordV1,
    advance: AddressAdvance,
}

/// Exact authorization and clock inputs for address-record validation.
#[derive(Clone, Copy, Debug)]
pub struct AddressRecordValidation<'a> {
    target: AddressRecordTarget,
    authorization: &'a SpaceAuthorizationView,
    now_ms: u64,
}

/// Exact Space and Endpoint expected at the address persistence boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRecordTarget {
    space_id: SpaceId,
    endpoint_id: EndpointId,
}

impl AddressRecordTarget {
    /// Creates one exact address-record persistence target.
    pub const fn new(space_id: SpaceId, endpoint_id: EndpointId) -> Self {
        Self {
            space_id,
            endpoint_id,
        }
    }
}

impl<'a> AddressRecordValidation<'a> {
    /// Creates one coherent address-record validation decision.
    pub const fn new(
        target: AddressRecordTarget,
        authorization: &'a SpaceAuthorizationView,
        now_ms: u64,
    ) -> Self {
        Self {
            target,
            authorization,
            now_ms,
        }
    }
}

impl ValidatedAddressRecord {
    /// Parses untrusted bytes and validates their exact Space, Endpoint, time, and signature.
    ///
    /// # Errors
    /// Returns [`AddressRecordBoundaryError`] for every rejected trust-boundary condition.
    pub fn parse(
        bytes: &[u8],
        validation: AddressRecordValidation<'_>,
    ) -> Result<Self, AddressRecordBoundaryError> {
        let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(bytes)
            .map_err(|_| AddressRecordBoundaryError::InvalidEncoding)?;
        let record = signed.record();
        if record.endpoint_id() != validation.target.endpoint_id {
            return Err(AddressRecordBoundaryError::WrongEndpoint);
        }
        if validation.authorization.space_id() != validation.target.space_id
            || record.space_id() != validation.target.space_id
        {
            return Err(AddressRecordBoundaryError::WrongSpace);
        }
        if !validation
            .authorization
            .contains_member(validation.target.endpoint_id)
        {
            return Err(AddressRecordBoundaryError::UnauthorizedMember);
        }
        if record.issued_at_ms() > validation.now_ms {
            return Err(AddressRecordBoundaryError::FutureRecord);
        }
        if record.expires_at_ms() <= validation.now_ms {
            return Err(AddressRecordBoundaryError::ExpiredRecord);
        }
        signed
            .verify_signature()
            .map_err(|_| AddressRecordBoundaryError::InvalidSignature)?;
        let issued_at_ms = i64::try_from(record.issued_at_ms())
            .map_err(|_| AddressRecordBoundaryError::InvalidEncoding)?;
        let expires_at_ms = i64::try_from(record.expires_at_ms())
            .map_err(|_| AddressRecordBoundaryError::InvalidEncoding)?;
        let advance = AddressAdvance {
            space_id: record.space_id(),
            endpoint_id: record.endpoint_id(),
            sequence: record.sequence(),
            issued_at_ms,
            expires_at_ms,
            record_hash: signed.record_hash(),
            signed_record: signed.canonical_bytes().to_vec(),
        };
        Ok(Self { signed, advance })
    }

    /// Returns the accepted signed record.
    pub const fn record(&self) -> &SignedSpaceAddressRecordV1 {
        &self.signed
    }

    pub(crate) const fn advance(&self) -> &AddressAdvance {
        &self.advance
    }
}

/// Rejection reason before address-record persistence is reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "network callers must map every closed address trust-boundary rejection"
)]
pub enum AddressRecordBoundaryError {
    /// Canonical record bytes are malformed, oversized, or out of storage range.
    InvalidEncoding,
    /// Record is not scoped to the expected Space.
    WrongSpace,
    /// Record is not signed for the expected Endpoint.
    WrongEndpoint,
    /// Endpoint is not a current member of the expected Space.
    UnauthorizedMember,
    /// Record issue time is in the future.
    FutureRecord,
    /// Record has expired.
    ExpiredRecord,
    /// Endpoint signature is invalid.
    InvalidSignature,
}

impl std::fmt::Display for AddressRecordBoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        }
    }
}

impl std::error::Error for AddressRecordBoundaryError {}

/// Typed outcome of a per-Space per-Endpoint address high-water update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every closed schema-v1 address sequence outcome"
)]
pub enum AddressRecordOutcome {
    /// A higher sequence was committed.
    Advanced {
        /// Revision committed with the higher sequence.
        revision: u64,
    },
    /// The identical record was already accepted.
    Idempotent {
        /// Highest accepted sequence.
        current_sequence: u64,
    },
    /// A lower sequence attempted to roll state back.
    Rollback {
        /// Highest accepted sequence.
        current_sequence: u64,
    },
    /// Different bytes attempted to reuse the accepted sequence.
    Fork {
        /// Highest accepted sequence.
        current_sequence: u64,
    },
}

impl Repository {
    /// Persists an address record that crossed the validated trust boundary.
    ///
    /// # Errors
    /// Returns [`StoreError`] when address state cannot be read or committed.
    pub fn advance_validated_address(
        &mut self,
        record: &ValidatedAddressRecord,
    ) -> Result<AddressRecordOutcome, StoreError> {
        self.advance_address(record.advance())
    }

    /// Replaces current address state only when the sequence increases.
    ///
    /// # Errors
    /// Returns [`StoreError`] when address state cannot be read or committed.
    fn advance_address(
        &mut self,
        advance: &AddressAdvance,
    ) -> Result<AddressRecordOutcome, StoreError> {
        let transaction = self.immediate()?;
        let current = transaction
            .query_row(
                "SELECT sequence, record_hash, signed_record FROM address_state
                 WHERE space_id = ?1 AND endpoint_id = ?2",
                (
                    advance.space_id.as_bytes().as_slice(),
                    advance.endpoint_id.as_bytes().as_slice(),
                ),
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((sequence, hash, signed)) = current {
            let outcome = match advance.sequence.cmp(&sequence) {
                std::cmp::Ordering::Less => AddressRecordOutcome::Rollback {
                    current_sequence: sequence,
                },
                std::cmp::Ordering::Equal
                    if hash.as_slice() == advance.record_hash
                        && signed == advance.signed_record =>
                {
                    AddressRecordOutcome::Idempotent {
                        current_sequence: sequence,
                    }
                }
                std::cmp::Ordering::Equal => AddressRecordOutcome::Fork {
                    current_sequence: sequence,
                },
                std::cmp::Ordering::Greater => {
                    return commit_address_advance(transaction, advance);
                }
            };
            return Ok(outcome);
        }
        commit_address_advance(transaction, advance)
    }
}

fn commit_address_advance(
    transaction: rusqlite::Transaction<'_>,
    advance: &AddressAdvance,
) -> Result<AddressRecordOutcome, StoreError> {
    transaction.execute(
        "INSERT INTO address_state(space_id, endpoint_id, sequence, issued_at_ms, expires_at_ms,
         record_hash, signed_record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(space_id, endpoint_id) DO UPDATE SET sequence = excluded.sequence,
         issued_at_ms = excluded.issued_at_ms, expires_at_ms = excluded.expires_at_ms,
         record_hash = excluded.record_hash, signed_record = excluded.signed_record",
        (
            advance.space_id.as_bytes().as_slice(),
            advance.endpoint_id.as_bytes().as_slice(),
            advance.sequence,
            advance.issued_at_ms,
            advance.expires_at_ms,
            advance.record_hash.as_slice(),
            advance.signed_record.as_slice(),
        ),
    )?;
    let revision = increment_revision(&transaction)?;
    transaction.commit()?;
    Ok(AddressRecordOutcome::Advanced { revision })
}
