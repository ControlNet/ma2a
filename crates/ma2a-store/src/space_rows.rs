use ma2a_core::{ManifestError, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceChain, SpaceId};
use rusqlite::{Connection, OptionalExtension as _, Transaction};

use crate::{KeyReference, StoreError};
mod derived;
use derived::{derived_members, derived_revocations, validate_derived_state};

// Local authority custody is incompatible with losing the genesis owner in
// Phase 1. This is a repository invariant, not a generic signed-chain rule.
fn validate_owned_owner(chain: &SpaceChain) -> Result<(), StoreError> {
    let owner = chain.genesis().genesis().initial_member().endpoint_id();
    if !chain
        .members()
        .iter()
        .any(|member| member.endpoint_id() == owner)
        || chain
            .revocations()
            .iter()
            .any(|revoked| revoked.endpoint_id() == owner)
    {
        return Err(StoreError::SchemaMismatch {
            detail: "locally owned Space has removed or revoked its genesis owner; operator action required",
        });
    }
    Ok(())
}

pub(super) fn load_chain(
    connection: &Connection,
    space_id: SpaceId,
) -> Result<Option<SpaceChain>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT genesis_cbor, latest_manifest_generation, latest_manifest_hash, authority_key_ref
             FROM spaces WHERE space_id = ?1",
            [space_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((genesis_bytes, stored_generation, stored_hash, authority)) = stored else {
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
    if authority.is_some() {
        validate_owned_owner(&chain)?;
    }
    validate_derived_state(connection, &chain)?;
    Ok(Some(chain))
}

pub(super) fn replace_chain(
    transaction: &Transaction<'_>,
    chain: &SpaceChain,
    authority_key_reference: Option<&KeyReference>,
) -> Result<(), StoreError> {
    let owned = authority_key_reference.is_some()
        || transaction
            .query_row(
                "SELECT authority_key_ref IS NOT NULL FROM spaces WHERE space_id = ?1",
                [chain.space_id().as_bytes().as_slice()],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
    if owned {
        validate_owned_owner(chain)?;
    }
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
