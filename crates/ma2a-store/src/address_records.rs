use rusqlite::OptionalExtension as _;

use crate::{AddressAdvance, Repository, StoreError, repository::increment_revision};

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
