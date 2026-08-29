use std::collections::BTreeMap;

use ma2a_core::{
    EndpointId, ManifestError, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceChain, SpaceId,
};
use rusqlite::{Connection, OptionalExtension as _, Transaction};

use crate::{KeyReference, StoreError};

pub(super) fn load_chain(
    connection: &Connection,
    space_id: SpaceId,
) -> Result<Option<SpaceChain>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT genesis_cbor, latest_manifest_generation, latest_manifest_hash
             FROM spaces WHERE space_id = ?1",
            [space_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((genesis_bytes, stored_generation, stored_hash)) = stored else {
        return Ok(None);
    };
    let genesis = SignedSpaceGenesisV1::from_canonical_bytes(&genesis_bytes)
        .map_err(|_| StoreError::Manifest(ManifestError::INVALID_SIGNATURE))?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    if chain.space_id() != space_id {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space identifier does not match genesis",
        });
    }
    let mut statement = connection.prepare(
        "SELECT generation, previous_hash, manifest_hash, signed_manifest
         FROM manifests WHERE space_id = ?1 ORDER BY generation",
    )?;
    let rows = statement.query_map([space_id.as_bytes().as_slice()], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, Vec<u8>>(3)?,
        ))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    let Some((generation, previous_hash, manifest_hash, signed_genesis)) = rows.first() else {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space genesis row is missing",
        });
    };
    if *generation != 0
        || previous_hash.is_some()
        || manifest_hash.as_slice() != chain.genesis().chain_hash()
        || signed_genesis != chain.genesis().canonical_bytes()
    {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space genesis row does not match genesis",
        });
    }
    for (stored_generation, stored_previous_hash, stored_manifest_hash, bytes) in
        rows.iter().skip(1)
    {
        let manifest =
            SignedSpaceManifestV1::from_canonical_bytes(bytes, chain.genesis().authority())
                .map_err(|_| StoreError::Manifest(ManifestError::INVALID_SIGNATURE))?;
        if *stored_generation != manifest.generation()
            || stored_previous_hash.as_deref() != Some(manifest.previous_hash().as_slice())
            || stored_manifest_hash.as_slice() != manifest.manifest_hash()
        {
            return Err(StoreError::SchemaMismatch {
                detail: "stored Space manifest metadata does not match signed bytes",
            });
        }
        chain.apply(&manifest)?;
    }
    if stored_generation != Some(chain.latest_generation())
        || stored_hash.as_deref() != Some(chain.latest_hash().as_slice())
    {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space latest state does not match its signed chain",
        });
    }
    validate_derived_state(connection, &chain)?;
    Ok(Some(chain))
}

pub(super) fn replace_chain(
    transaction: &Transaction<'_>,
    chain: &SpaceChain,
    authority_key_reference: Option<&KeyReference>,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO spaces(space_id, genesis_cbor, authority_key_ref, latest_manifest_generation, latest_manifest_hash)
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(space_id) DO UPDATE SET
         genesis_cbor = excluded.genesis_cbor,
         authority_key_ref = COALESCE(excluded.authority_key_ref, spaces.authority_key_ref),
         latest_manifest_generation = excluded.latest_manifest_generation,
         latest_manifest_hash = excluded.latest_manifest_hash",
        (
            chain.space_id().as_bytes().as_slice(),
            chain.genesis().canonical_bytes(),
            authority_key_reference.map(KeyReference::as_str),
            chain.latest_generation(),
            chain.latest_hash().as_slice(),
        ),
    )?;
    transaction.execute(
        "DELETE FROM manifests WHERE space_id = ?1",
        [chain.space_id().as_bytes().as_slice()],
    )?;
    insert_manifest_rows(transaction, chain)?;
    insert_derived_state(transaction, chain)
}

fn insert_manifest_rows(
    transaction: &Transaction<'_>,
    chain: &SpaceChain,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO manifests(space_id, generation, previous_hash, manifest_hash, signed_manifest)
         VALUES (?1, 0, NULL, ?2, ?3)",
        (
            chain.space_id().as_bytes().as_slice(),
            chain.genesis().chain_hash().as_slice(),
            chain.genesis().canonical_bytes(),
        ),
    )?;
    for manifest in chain.manifests() {
        transaction.execute(
            "INSERT INTO manifests(space_id, generation, previous_hash, manifest_hash, signed_manifest)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                chain.space_id().as_bytes().as_slice(),
                manifest.generation(),
                manifest.previous_hash().as_slice(),
                manifest.manifest_hash().as_slice(),
                manifest.canonical_bytes(),
            ),
        )?;
    }
    Ok(())
}

fn insert_derived_state(
    transaction: &Transaction<'_>,
    chain: &SpaceChain,
) -> Result<(), StoreError> {
    for (endpoint_id, (role, accepted_generation)) in derived_members(chain) {
        transaction.execute(
            "INSERT INTO members(space_id, endpoint_id, role, accepted_generation) VALUES (?1, ?2, ?3, ?4)",
            (chain.space_id().as_bytes().as_slice(), endpoint_id.as_bytes().as_slice(), role, accepted_generation),
        )?;
    }
    for (endpoint_id, (generation, signed_revocation)) in derived_revocations(chain) {
        transaction.execute(
            "INSERT INTO member_revocations(space_id, endpoint_id, revoked_generation, signed_revocation)
             VALUES (?1, ?2, ?3, ?4)",
            (chain.space_id().as_bytes().as_slice(), endpoint_id.as_bytes().as_slice(), generation, signed_revocation),
        )?;
    }
    Ok(())
}

fn derived_members(chain: &SpaceChain) -> BTreeMap<EndpointId, (u8, u64)> {
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

fn derived_revocations(chain: &SpaceChain) -> BTreeMap<EndpointId, (u64, Vec<u8>)> {
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

fn validate_derived_state(connection: &Connection, chain: &SpaceChain) -> Result<(), StoreError> {
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
