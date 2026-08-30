use std::{
    cmp::Ordering,
    collections::{BTreeMap, btree_map::Entry},
};

use ma2a_core::{EndpointId, SpaceChain, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::{
    KeyReference, Repository, StoreError, ValidatedAddressRecord, ValidatedRelayAdvertisement,
    repository::increment_revision, space_rows::replace_chain,
};

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
        let transaction = self.immediate()?;
        let address_writes = validate_addresses(&transaction, &batch.addresses)?;
        let relay_writes = validate_relays(&transaction, &batch.relays)?;
        let mut changed = !batch.chains.is_empty();
        for chain in &batch.chains {
            let reference = transaction
                .query_row(
                    "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                    [chain.space_id().as_bytes().as_slice()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten()
                .map(|value| KeyReference::parse(&value))
                .transpose()?;
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
        if !changed {
            return Ok(None);
        }
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(Some(revision))
    }
}

type HighWater = (u64, Vec<u8>, Vec<u8>);

fn validate_addresses(
    transaction: &rusqlite::Transaction<'_>,
    records: &[ValidatedAddressRecord],
) -> Result<Vec<bool>, StoreError> {
    let mut state = BTreeMap::<(SpaceId, EndpointId), HighWater>::new();
    let mut writes = Vec::with_capacity(records.len());
    for record in records {
        let advance = record.advance();
        let key = (advance.space_id, advance.endpoint_id);
        if let Entry::Vacant(entry) = state.entry(key) {
            let current = transaction
                .query_row(
                    "SELECT sequence, record_hash, signed_record FROM address_state
                     WHERE space_id = ?1 AND endpoint_id = ?2",
                    (
                        advance.space_id.as_bytes().as_slice(),
                        advance.endpoint_id.as_bytes().as_slice(),
                    ),
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            if let Some(current) = current {
                entry.insert(current);
            }
        }
        let candidate = (
            advance.sequence,
            advance.record_hash.to_vec(),
            advance.signed_record.clone(),
        );
        writes.push(validate_high_water(state.get(&key), &candidate)?);
        state.insert(key, candidate);
    }
    Ok(writes)
}

fn validate_relays(
    transaction: &rusqlite::Transaction<'_>,
    records: &[ValidatedRelayAdvertisement],
) -> Result<Vec<bool>, StoreError> {
    let mut state = BTreeMap::<(SpaceId, EndpointId), HighWater>::new();
    let mut writes = Vec::with_capacity(records.len());
    for record in records {
        let key = (record.space_id(), record.provider_endpoint_id());
        if let Entry::Vacant(entry) = state.entry(key) {
            let current = transaction
                .query_row(
                    "SELECT sequence, advertisement_hash, signed_advertisement
                     FROM relay_advertisement_state
                     WHERE space_id = ?1 AND relay_endpoint_id = ?2",
                    (
                        record.space_id().as_bytes().as_slice(),
                        record.provider_endpoint_id().as_bytes().as_slice(),
                    ),
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            if let Some(current) = current {
                entry.insert(current);
            }
        }
        let candidate = (
            record.sequence(),
            record.advertisement_hash().to_vec(),
            record.signed_advertisement().to_vec(),
        );
        writes.push(validate_high_water(state.get(&key), &candidate)?);
        state.insert(key, candidate);
    }
    Ok(writes)
}

fn validate_high_water(
    current: Option<&HighWater>,
    candidate: &HighWater,
) -> Result<bool, StoreError> {
    let Some(current) = current else {
        return Ok(true);
    };
    match candidate.0.cmp(&current.0) {
        Ordering::Equal if candidate.1 == current.1 && candidate.2 == current.2 => Ok(false),
        Ordering::Less | Ordering::Equal => Err(StoreError::ControlConflict),
        Ordering::Greater => Ok(true),
    }
}
