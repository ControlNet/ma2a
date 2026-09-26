use ma2a_core::SpaceChain;
use rusqlite::OptionalExtension as _;

use crate::{
    KeyReference, Repository, StoreError, ValidatedAddressRecord, ValidatedRelayAdvertisement,
    repository::increment_revision, space_rows::replace_chain,
};

use self::validation::{validate_addresses, validate_chains, validate_relays};

mod validation;

/// Fully validated control artifacts committed under one Runtime revision.
#[derive(Clone, Debug)]
pub struct ControlBatch {
    chains: Vec<SpaceChain>,
    addresses: Vec<ValidatedAddressRecord>,
    relays: Vec<ValidatedRelayAdvertisement>,
}

impl ControlBatch {
    /// Collects already validated control artifacts for one atomic commit.
    pub const fn new(
        chains: Vec<SpaceChain>,
        addresses: Vec<ValidatedAddressRecord>,
        relays: Vec<ValidatedRelayAdvertisement>,
    ) -> Self {
        Self {
            chains,
            addresses,
            relays,
        }
    }
}

impl Repository {
    /// Persists one complete validated control batch in a single transaction.
    ///
    /// # Errors
    /// Returns [`StoreError`] when any row cannot be written or committed.
    pub fn persist_control_batch(
        &mut self,
        batch: &ControlBatch,
    ) -> Result<Option<u64>, StoreError> {
        if batch.chains.is_empty() && batch.addresses.is_empty() && batch.relays.is_empty() {
            return Ok(None);
        }
        self.persist_control_batch_projected(batch, |_| Ok(()))
            .map(|(changed, _)| changed)
    }

    pub(crate) fn persist_control_batch_projected<T>(
        &mut self,
        batch: &ControlBatch,
        project: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<(Option<u64>, crate::Committed<T>), StoreError> {
        let transaction = self.immediate()?;
        let chain_writes = validate_chains(&transaction, &batch.chains)?;
        let address_writes = validate_addresses(&transaction, &batch.addresses)?;
        let relay_writes = validate_relays(&transaction, &batch.relays)?;
        let chain_references = batch
            .chains
            .iter()
            .map(|chain| {
                transaction
                    .query_row(
                        "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                        [chain.space_id().as_bytes().as_slice()],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten()
                    .map(|value| KeyReference::parse(&value))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut changed = false;
        for ((chain, write), reference) in
            batch.chains.iter().zip(chain_writes).zip(chain_references)
        {
            if !write {
                continue;
            }
            changed = true;
            replace_chain(&transaction, chain, reference.as_ref())?;
        }
        for (record, write) in batch.addresses.iter().zip(address_writes) {
            if !write {
                continue;
            }
            changed = true;
            let advance = record.advance();
            transaction.execute(
                "INSERT INTO address_state(space_id, endpoint_id, sequence, issued_at_ms,
                 expires_at_ms, record_hash, signed_record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
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
        }
        for (advance, write) in batch.relays.iter().zip(relay_writes) {
            if !write {
                continue;
            }
            changed = true;
            transaction.execute(
                "INSERT INTO relay_advertisement_state(space_id, relay_endpoint_id, sequence,
                 issued_at_ms, expires_at_ms, advertisement_hash, signed_advertisement)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(space_id, relay_endpoint_id) DO UPDATE SET sequence = excluded.sequence,
                 issued_at_ms = excluded.issued_at_ms, expires_at_ms = excluded.expires_at_ms,
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
        }
        let revision = if changed {
            increment_revision(&transaction)?
        } else {
            transaction.query_row(
                "SELECT revision FROM runtime_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?
        };
        let value = project(&transaction)?;
        transaction.commit()?;
        Ok((
            changed.then_some(revision),
            crate::Committed::new(revision, value),
        ))
    }
}
