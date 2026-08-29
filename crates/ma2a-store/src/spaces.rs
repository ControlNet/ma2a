use std::collections::BTreeSet;

use ma2a_core::{EndpointId, ManifestError, SpaceAuthoritySecret, SpaceChain, SpaceId};
use rusqlite::OptionalExtension as _;

use crate::{
    KeyKind, KeyReference, Repository, StoreError,
    repository::increment_revision,
    space_rows::{load_chain, replace_chain},
};

/// Result of comparing and atomically persisting a complete Space chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceChainPersistence {
    revision: Option<u64>,
    error: Option<ManifestError>,
}

impl SpaceChainPersistence {
    /// Returns whether this operation committed a newer chain.
    pub const fn advanced(self) -> bool {
        self.revision.is_some()
    }

    /// Returns the rejected transition, if the proposed chain regressed or forked.
    pub const fn error(self) -> Option<ManifestError> {
        self.error
    }

    /// Returns the revision committed by an advancement.
    pub const fn revision(self) -> Option<u64> {
        self.revision
    }

    const fn advanced_at(revision: u64) -> Self {
        Self {
            revision: Some(revision),
            error: None,
        }
    }

    const fn idempotent() -> Self {
        Self {
            revision: None,
            error: None,
        }
    }

    const fn rejected(error: ManifestError) -> Self {
        Self {
            revision: None,
            error: Some(error),
        }
    }
}

impl Repository {
    /// Loads every verified Space in which the Endpoint is currently a member.
    ///
    /// # Errors
    /// Returns [`StoreError`] when persisted membership rows are malformed.
    pub fn memberships_for(
        &self,
        endpoint_id: EndpointId,
    ) -> Result<BTreeSet<SpaceId>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT space_id FROM members WHERE endpoint_id = ?1 ORDER BY space_id")?;
        statement
            .query_map([endpoint_id.as_bytes().as_slice()], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .map(|row| {
                let bytes = row?;
                SpaceId::try_from(bytes.as_slice()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "persisted membership Space identifier is invalid",
                })
            })
            .collect()
    }
    pub(crate) fn validate_space_chains(&self) -> Result<(), StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT space_id, authority_key_ref FROM spaces ORDER BY space_id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (space_id, reference) = row?;
            let space_id =
                SpaceId::try_from(space_id.as_slice()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "stored Space identifier is invalid",
                })?;
            let chain =
                load_chain(&self.connection, space_id)?.ok_or(StoreError::SchemaMismatch {
                    detail: "stored Space chain is missing",
                })?;
            if let Some(value) = reference {
                let reference = KeyReference::parse(&value)?;
                let protected = self.key_store.read(KeyKind::SpaceAuthority, &reference)?;
                validate_authority(&chain, &protected)?;
            }
        }
        Ok(())
    }

    /// Atomically stores a complete verified Space chain and its latest derived state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when protected-key validation or `SQLite` access fails.
    pub fn persist_space_chain(
        &mut self,
        chain: &SpaceChain,
    ) -> Result<SpaceChainPersistence, StoreError> {
        self.persist_space_chain_with_authority(chain, None)
    }

    pub(crate) fn persist_space_chain_with_authority(
        &mut self,
        chain: &SpaceChain,
        authority_key_reference: Option<&KeyReference>,
    ) -> Result<SpaceChainPersistence, StoreError> {
        let stored_reference = self
            .connection
            .query_row(
                "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                [chain.space_id().as_bytes().as_slice()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|value| KeyReference::parse(&value))
            .transpose()?;
        if authority_key_reference.is_some()
            && stored_reference.is_some()
            && authority_key_reference != stored_reference.as_ref()
        {
            return Err(StoreError::SchemaMismatch {
                detail: "Space authority key reference cannot be replaced",
            });
        }
        let effective_reference = authority_key_reference.cloned().or(stored_reference);
        if let Some(reference) = &effective_reference {
            let protected = self.key_store.read(KeyKind::SpaceAuthority, reference)?;
            validate_authority(chain, &protected)?;
        }
        let transaction = self.immediate()?;
        let transaction_reference = transaction
            .query_row(
                "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                [chain.space_id().as_bytes().as_slice()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|value| KeyReference::parse(&value))
            .transpose()?;
        if transaction_reference.is_some() && transaction_reference != effective_reference {
            return Err(StoreError::SchemaMismatch {
                detail: "Space authority key reference changed before persistence",
            });
        }
        let stored = load_chain(&transaction, chain.space_id())?;
        if let Some(existing) = &stored {
            if existing.genesis() != chain.genesis() {
                return Ok(SpaceChainPersistence::rejected(ManifestError::FORK));
            }
            if chain.latest_generation() < existing.latest_generation() {
                let error = if existing.manifests().starts_with(chain.manifests()) {
                    ManifestError::ROLLBACK
                } else {
                    ManifestError::FORK
                };
                return Ok(SpaceChainPersistence::rejected(error));
            }
            if chain.latest_generation() == existing.latest_generation() {
                return Ok(if chain == existing {
                    SpaceChainPersistence::idempotent()
                } else {
                    SpaceChainPersistence::rejected(ManifestError::FORK)
                });
            }
            if !chain.manifests().starts_with(existing.manifests()) {
                return Ok(SpaceChainPersistence::rejected(ManifestError::FORK));
            }
        }
        replace_chain(&transaction, chain, effective_reference.as_ref())?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(SpaceChainPersistence::advanced_at(revision))
    }

    /// Loads and verifies one complete Space chain from canonical signed rows.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when database rows are invalid or inaccessible.
    pub fn load_space_chain(&self, space_id: SpaceId) -> Result<Option<SpaceChain>, StoreError> {
        load_chain(&self.connection, space_id)
    }

    /// Exports one stored Space chain without local authority-key material.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the chain cannot be loaded or encoded.
    pub fn export_space_chain(&self, space_id: SpaceId) -> Result<Option<Vec<u8>>, StoreError> {
        self.load_space_chain(space_id)?
            .map(|chain| {
                chain
                    .export_public()
                    .map_err(|_| StoreError::Manifest(ManifestError::INVALID_ENCODING))
            })
            .transpose()
    }

    /// Imports and atomically persists a public signed Space chain.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when validation or persistence fails.
    pub fn import_space_chain(
        &mut self,
        bytes: &[u8],
    ) -> Result<SpaceChainPersistence, StoreError> {
        let chain = SpaceChain::import_public(bytes)?;
        self.persist_space_chain(&chain)
    }
}

fn validate_authority(
    chain: &SpaceChain,
    protected: &crate::ProtectedSecret,
) -> Result<(), StoreError> {
    let secret = SpaceAuthoritySecret::try_from_bytes(protected.as_bytes()).map_err(|_| {
        StoreError::SchemaMismatch {
            detail: "stored Space authority key has invalid length",
        }
    })?;
    if secret.public_key() != chain.genesis().authority() {
        return Err(StoreError::SchemaMismatch {
            detail: "stored Space authority key does not match genesis",
        });
    }
    Ok(())
}
