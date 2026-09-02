use ma2a_core::{EndpointId, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::repository::increment_revision;
use crate::{Repository, StoreError, ValidatedRelayAdvertisement};

/// Persisted signed private relay advertisement for one Space and provider Endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedRelayAdvertisement {
    space_id: SpaceId,
    provider_endpoint_id: EndpointId,
    sequence: u64,
    issued_at_ms: i64,
    expires_at_ms: i64,
    active: bool,
    advertisement_hash: [u8; 32],
    signed_advertisement: Vec<u8>,
}

impl PersistedRelayAdvertisement {
    /// Returns the exact Space scope.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns the provider Endpoint identity.
    pub const fn provider_endpoint_id(&self) -> EndpointId {
        self.provider_endpoint_id
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

    /// Returns whether consumers may use this accepted advertisement.
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the signed advertisement hash.
    pub const fn advertisement_hash(&self) -> [u8; 32] {
        self.advertisement_hash
    }

    /// Returns exact signed canonical bytes.
    pub fn signed_advertisement(&self) -> &[u8] {
        &self.signed_advertisement
    }
}

/// Typed outcome of a per-Space per-provider advertisement high-water update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every closed schema-v1 relay advertisement outcome"
)]
pub enum RelayAdvertisementOutcome {
    /// A higher sequence was committed.
    Advanced {
        /// Revision committed with the higher sequence.
        revision: u64,
    },
    /// The identical advertisement was already accepted.
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
    /// Advances private relay advertisement state with rollback and fork detection.
    ///
    /// # Errors
    /// Returns [`StoreError`] when advertisement state cannot be read or committed.
    pub fn advance_private_relay_advertisement(
        &mut self,
        advance: &ValidatedRelayAdvertisement,
    ) -> Result<RelayAdvertisementOutcome, StoreError> {
        let transaction = self.immediate()?;
        let current = transaction
            .query_row(
                "SELECT sequence, advertisement_hash, signed_advertisement
                 FROM relay_advertisement_state
                 WHERE space_id = ?1 AND relay_endpoint_id = ?2",
                (
                    advance.space_id().as_bytes().as_slice(),
                    advance.provider_endpoint_id().as_bytes().as_slice(),
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
            let outcome = match advance.sequence().cmp(&sequence) {
                std::cmp::Ordering::Less => RelayAdvertisementOutcome::Rollback {
                    current_sequence: sequence,
                },
                std::cmp::Ordering::Equal
                    if hash.as_slice() == advance.advertisement_hash()
                        && signed == advance.signed_advertisement() =>
                {
                    RelayAdvertisementOutcome::Idempotent {
                        current_sequence: sequence,
                    }
                }
                std::cmp::Ordering::Equal => RelayAdvertisementOutcome::Fork {
                    current_sequence: sequence,
                },
                std::cmp::Ordering::Greater => {
                    return commit_advertisement(transaction, advance);
                }
            };
            return Ok(outcome);
        }
        commit_advertisement(transaction, advance)
    }

    /// Loads the current signed advertisement for one exact Space and provider.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed or cannot be read.
    pub fn relay_advertisement(
        &self,
        space_id: SpaceId,
        provider_endpoint_id: EndpointId,
    ) -> Result<Option<PersistedRelayAdvertisement>, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT sequence, issued_at_ms, expires_at_ms, active,
                        advertisement_hash, signed_advertisement
                 FROM relay_advertisement_state
                 WHERE space_id = ?1 AND relay_endpoint_id = ?2",
                (
                    space_id.as_bytes().as_slice(),
                    provider_endpoint_id.as_bytes().as_slice(),
                ),
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(sequence, issued_at_ms, expires_at_ms, active, hash, signed_advertisement)| {
                let advertisement_hash =
                    <[u8; 32]>::try_from(hash).map_err(|_| StoreError::SchemaMismatch {
                        detail: "persisted relay advertisement hash has invalid length",
                    })?;
                Ok(PersistedRelayAdvertisement {
                    space_id,
                    provider_endpoint_id,
                    sequence,
                    issued_at_ms,
                    expires_at_ms,
                    active,
                    advertisement_hash,
                    signed_advertisement,
                })
            },
        )
        .transpose()
    }

    /// Loads every current signed relay advertisement in one exact Space.
    ///
    /// # Errors
    /// Returns [`StoreError`] when any row is malformed or cannot be read.
    pub fn relay_advertisements_for_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<PersistedRelayAdvertisement>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT relay_endpoint_id, sequence, issued_at_ms, expires_at_ms, active,
                    advertisement_hash, signed_advertisement
             FROM relay_advertisement_state WHERE space_id = ?1 ORDER BY relay_endpoint_id",
        )?;
        let rows = statement.query_map([space_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Vec<u8>>(5)?,
                row.get::<_, Vec<u8>>(6)?,
            ))
        })?;
        let mut advertisements = Vec::new();
        for row in rows {
            let (
                provider,
                sequence,
                issued_at_ms,
                expires_at_ms,
                active,
                hash,
                signed_advertisement,
            ) = row?;
            advertisements.push(PersistedRelayAdvertisement {
                space_id,
                provider_endpoint_id: EndpointId::try_from(provider.as_slice()).map_err(|_| {
                    StoreError::SchemaMismatch {
                        detail: "persisted relay provider identifier is invalid",
                    }
                })?,
                sequence,
                issued_at_ms,
                expires_at_ms,
                active,
                advertisement_hash: <[u8; 32]>::try_from(hash).map_err(|_| {
                    StoreError::SchemaMismatch {
                        detail: "persisted relay advertisement hash has invalid length",
                    }
                })?,
                signed_advertisement,
            });
        }
        Ok(advertisements)
    }
}

fn commit_advertisement(
    transaction: rusqlite::Transaction<'_>,
    advance: &ValidatedRelayAdvertisement,
) -> Result<RelayAdvertisementOutcome, StoreError> {
    transaction.execute(
        "INSERT INTO relay_advertisement_state(space_id, relay_endpoint_id, sequence,
         issued_at_ms, expires_at_ms, active, advertisement_hash, signed_advertisement)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
         ON CONFLICT(space_id, relay_endpoint_id) DO UPDATE SET sequence = excluded.sequence,
         issued_at_ms = excluded.issued_at_ms, expires_at_ms = excluded.expires_at_ms,
         active = 1,
         advertisement_hash = excluded.advertisement_hash,
         signed_advertisement = excluded.signed_advertisement",
        (
            advance.space_id().as_bytes().as_slice(),
            advance.provider_endpoint_id().as_bytes().as_slice(),
            advance.sequence(),
            advance.issued_at_ms(),
            advance.expires_at_ms(),
            advance.advertisement_hash().as_slice(),
            advance.signed_advertisement(),
        ),
    )?;
    let revision = increment_revision(&transaction)?;
    transaction.commit()?;
    Ok(RelayAdvertisementOutcome::Advanced { revision })
}
