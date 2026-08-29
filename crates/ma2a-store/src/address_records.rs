use ma2a_core::{EndpointId, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::{AddressAdvance, Repository, StoreError, repository::increment_revision};

/// Persisted signed address record for one Space and Endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedAddressRecord {
    space_id: SpaceId,
    endpoint_id: EndpointId,
    sequence: u64,
    issued_at_ms: i64,
    expires_at_ms: i64,
    record_hash: [u8; 32],
    signed_record: Vec<u8>,
}

impl PersistedAddressRecord {
    /// Returns the Space scope.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }
    /// Returns the Endpoint subject.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }
    /// Returns the accepted high-water sequence.
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Returns the signed issue timestamp.
    pub const fn issued_at_ms(&self) -> i64 {
        self.issued_at_ms
    }
    /// Returns the signed expiry timestamp.
    pub const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
    /// Returns the signed record hash.
    pub const fn record_hash(&self) -> [u8; 32] {
        self.record_hash
    }
    /// Returns exact signed canonical bytes.
    pub fn signed_record(&self) -> &[u8] {
        &self.signed_record
    }
}

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
    /// Replaces current address state only when the sequence increases.
    ///
    /// # Errors
    /// Returns [`StoreError`] when address state cannot be read or committed.
    pub fn advance_address(
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

    /// Loads the current signed address record for one exact Space and Endpoint.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed or cannot be read.
    pub fn address_record(
        &self,
        space_id: SpaceId,
        endpoint_id: EndpointId,
    ) -> Result<Option<PersistedAddressRecord>, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT sequence, issued_at_ms, expires_at_ms, record_hash, signed_record
                 FROM address_state WHERE space_id = ?1 AND endpoint_id = ?2",
                (
                    space_id.as_bytes().as_slice(),
                    endpoint_id.as_bytes().as_slice(),
                ),
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(sequence, issued, expires, hash, signed)| {
            persisted_record(
                space_id,
                endpoint_id,
                sequence,
                issued,
                expires,
                &hash,
                signed,
            )
        })
        .transpose()
    }

    /// Loads every Space-local current record for one Endpoint.
    ///
    /// # Errors
    /// Returns [`StoreError`] when any row is malformed or cannot be read.
    pub fn address_records_for_endpoint(
        &self,
        endpoint_id: EndpointId,
    ) -> Result<Vec<PersistedAddressRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT space_id, sequence, issued_at_ms, expires_at_ms, record_hash, signed_record
             FROM address_state WHERE endpoint_id = ?1 ORDER BY space_id",
        )?;
        let rows = statement.query_map([endpoint_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (space, sequence, issued, expires, hash, signed) = row?;
            let space_id =
                SpaceId::try_from(space.as_slice()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "persisted address Space identifier is invalid",
                })?;
            records.push(persisted_record(
                space_id,
                endpoint_id,
                sequence,
                issued,
                expires,
                &hash,
                signed,
            )?);
        }
        Ok(records)
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

#[expect(
    clippy::too_many_arguments,
    reason = "the persisted schema-v1 address row has seven independent stored fields"
)]
fn persisted_record(
    space_id: SpaceId,
    endpoint_id: EndpointId,
    sequence: u64,
    issued_at_ms: i64,
    expires_at_ms: i64,
    hash: &[u8],
    signed_record: Vec<u8>,
) -> Result<PersistedAddressRecord, StoreError> {
    let record_hash = <[u8; 32]>::try_from(hash).map_err(|_| StoreError::SchemaMismatch {
        detail: "persisted address record hash has invalid length",
    })?;
    Ok(PersistedAddressRecord {
        space_id,
        endpoint_id,
        sequence,
        issued_at_ms,
        expires_at_ms,
        record_hash,
        signed_record,
    })
}
