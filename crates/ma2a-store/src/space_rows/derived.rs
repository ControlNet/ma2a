use crate::StoreError;
use ma2a_core::{EndpointId, SpaceChain};
use rusqlite::Connection;
use std::collections::BTreeMap;

pub(super) fn derived_members(chain: &SpaceChain) -> BTreeMap<EndpointId, (u8, u64)> {
    let owner = chain.genesis().genesis().initial_member().endpoint_id();
    let mut members = BTreeMap::from([(owner, (0, 0))]);
    for manifest in chain.manifests() {
        members.retain(|endpoint_id, _| {
            manifest
                .members()
                .binary_search_by_key(endpoint_id, ma2a_core::SpaceMemberV1::endpoint_id)
                .is_ok()
        });
        for member in manifest.members() {
            members.entry(member.endpoint_id()).or_insert_with(|| {
                (
                    if member.endpoint_id() == owner { 0 } else { 2 },
                    manifest.generation(),
                )
            });
        }
    }
    members
}

pub(super) fn derived_revocations(chain: &SpaceChain) -> BTreeMap<EndpointId, (u64, Vec<u8>)> {
    let mut revoked = BTreeMap::new();
    for manifest in chain.manifests() {
        for member in manifest.members() {
            revoked.remove(&member.endpoint_id());
        }
        for revocation in manifest.revocations() {
            revoked
                .entry(revocation.endpoint_id())
                .or_insert_with(|| (manifest.generation(), manifest.canonical_bytes().to_vec()));
        }
    }
    revoked
}

pub(super) fn validate_derived_state(
    connection: &Connection,
    chain: &SpaceChain,
) -> Result<(), StoreError> {
    let mut member_statement = connection.prepare(
        "SELECT endpoint_id, role, accepted_generation FROM members
         WHERE space_id = ?1 ORDER BY endpoint_id",
    )?;
    let stored_members = member_statement
        .query_map([chain.space_id().as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_members = derived_members(chain)
        .into_iter()
        .map(|(endpoint_id, (role, generation))| {
            (endpoint_id.as_bytes().to_vec(), role, generation)
        })
        .collect::<Vec<_>>();
    if stored_members != expected_members {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space members do not match signed chain",
        });
    }
    let mut revocation_statement = connection.prepare(
        "SELECT endpoint_id, revoked_generation, signed_revocation FROM member_revocations
         WHERE space_id = ?1 ORDER BY endpoint_id",
    )?;
    let stored_revocations = revocation_statement
        .query_map([chain.space_id().as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_revocations = derived_revocations(chain)
        .into_iter()
        .map(|(endpoint_id, (generation, bytes))| {
            (endpoint_id.as_bytes().to_vec(), generation, bytes)
        })
        .collect::<Vec<_>>();
    if stored_revocations != expected_revocations {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space revocations do not match signed chain",
        });
    }
    Ok(())
}
