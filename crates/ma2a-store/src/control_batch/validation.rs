use std::{
    cmp::Ordering,
    collections::{BTreeMap, btree_map::Entry},
};

use ma2a_core::{EndpointId, SpaceChain, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::{
    StoreError, ValidatedAddressRecord, ValidatedRelayAdvertisement, space_rows::load_chain,
};

type HighWater = (u64, Vec<u8>, Vec<u8>);

pub(super) fn validate_chains(
    transaction: &rusqlite::Transaction<'_>,
    chains: &[SpaceChain],
) -> Result<Vec<bool>, StoreError> {
    let mut state = BTreeMap::<SpaceId, SpaceChain>::new();
    let mut writes = Vec::with_capacity(chains.len());
    for chain in chains {
        let space_id = chain.space_id();
        if let Entry::Vacant(entry) = state.entry(space_id)
            && let Some(current) = load_chain(transaction, space_id)?
        {
            entry.insert(current);
        }
        writes.push(validate_chain_high_water(state.get(&space_id), chain)?);
        state.insert(space_id, chain.clone());
    }
    Ok(writes)
}

fn validate_chain_high_water(
    current: Option<&SpaceChain>,
    candidate: &SpaceChain,
) -> Result<bool, StoreError> {
    let Some(current) = current else {
        return Ok(true);
    };
    if current.genesis() != candidate.genesis() {
        return Err(StoreError::ControlConflict);
    }
    match candidate
        .latest_generation()
        .cmp(&current.latest_generation())
    {
        Ordering::Equal if candidate == current => Ok(false),
        Ordering::Greater if candidate.manifests().starts_with(current.manifests()) => Ok(true),
        Ordering::Less | Ordering::Equal | Ordering::Greater => Err(StoreError::ControlConflict),
    }
}

pub(super) fn validate_addresses(
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

pub(super) fn validate_relays(
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
