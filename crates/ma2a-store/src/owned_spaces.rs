use ma2a_core::{
    SpaceAuthoritySecret, SpaceAuthorizationView, SpaceChain, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceId, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1,
    SpacePolicyV1,
};
use zeroize::Zeroizing;

use crate::{KeyKind, KeyMaterial, KeyReference, Repository, StoreError};

/// Inputs for creating one repository-owned Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceCreation {
    created_at_ms: u64,
    initial_member: SpaceMemberV1,
    policy: SpacePolicyV1,
    name: Option<String>,
}

impl SpaceCreation {
    /// Creates one repository-owned Space request without a shared Space name.
    pub const fn new(
        created_at_ms: u64,
        initial_member: SpaceMemberV1,
        policy: SpacePolicyV1,
    ) -> Self {
        Self {
            created_at_ms,
            initial_member,
            policy,
            name: None,
        }
    }

    /// Binds the shared Space name signed into genesis and read by every member.
    #[must_use]
    pub fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
}

/// Committed public state returned after repository-owned Space creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedSpace {
    revision: u64,
    chain: SpaceChain,
    authorization: SpaceAuthorizationView,
}

/// Complete membership proposal for the next manifest of a repository-owned Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedSpaceUpdate {
    space_id: SpaceId,
    issued_at_ms: u64,
    membership: SpaceManifestMembership,
}

impl OwnedSpaceUpdate {
    /// Creates the next owner-signed membership proposal.
    pub const fn new(
        space_id: SpaceId,
        issued_at_ms: u64,
        membership: SpaceManifestMembership,
    ) -> Self {
        Self {
            space_id,
            issued_at_ms,
            membership,
        }
    }
}

/// Public state returned only after an owner-signed manifest commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvancedOwnedSpace {
    revision: u64,
    chain: SpaceChain,
    authorization: SpaceAuthorizationView,
}

impl AdvancedOwnedSpace {
    /// Returns the committed repository revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the complete committed signed chain.
    pub const fn chain(&self) -> &SpaceChain {
        &self.chain
    }

    /// Returns authorization derived after the transaction committed.
    pub const fn authorization(&self) -> &SpaceAuthorizationView {
        &self.authorization
    }
}

impl CreatedSpace {
    /// Returns the committed Space identifier.
    pub fn space_id(&self) -> SpaceId {
        self.chain.space_id()
    }

    /// Returns the committed signed public chain.
    pub const fn chain(&self) -> &SpaceChain {
        &self.chain
    }

    /// Returns authorization derived after the complete chain commit.
    pub const fn authorization(&self) -> &SpaceAuthorizationView {
        &self.authorization
    }

    /// Returns the repository revision committed with creation.
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

impl Repository {
    /// Generates, protects, signs, and atomically commits one new Space.
    ///
    /// # Errors
    /// Returns [`StoreError`] when randomness, protected publication, signing, or persistence fails.
    pub fn create_owned_space(
        &mut self,
        creation: &SpaceCreation,
    ) -> Result<CreatedSpace, StoreError> {
        let secret = SpaceAuthoritySecret::random()?;
        let genesis = SpaceGenesisV1::create(
            creation.created_at_ms,
            secret.public_key(),
            SpaceGenesisOwner::new(creation.initial_member.clone(), creation.policy),
        )?;
        let genesis = match &creation.name {
            Some(name) => genesis.with_name(name)?,
            None => genesis,
        }
        .sign(&secret)?;
        let chain = SpaceChain::from_genesis(genesis)?;
        let mut reference = String::with_capacity(70);
        reference.push_str("space-");
        for byte in chain.space_id().as_bytes() {
            use std::fmt::Write as _;
            write!(&mut reference, "{byte:02x}")
                .map_err(|_| StoreError::Protocol(ma2a_core::ProtocolError::INTERNAL))?;
        }
        let reference = KeyReference::parse(&reference)?;
        let secret_bytes = Zeroizing::new(secret.secret_bytes());
        self.key_store.write(KeyMaterial::new(
            KeyKind::SpaceAuthority,
            &reference,
            secret_bytes.as_slice(),
        ))?;
        let persistence = self.persist_space_chain_with_authority(&chain, Some(&reference))?;
        let revision = persistence.revision().ok_or(StoreError::SchemaMismatch {
            detail: "fresh Space creation did not advance repository state",
        })?;
        let authorization = SpaceAuthorizationView::from_chain(&chain);
        Ok(CreatedSpace {
            revision,
            chain,
            authorization,
        })
    }

    /// Signs and atomically commits the next manifest for a repository-owned Space.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the Space, protected authority, proposal, or commit is invalid.
    pub fn advance_owned_space(
        &mut self,
        update: &OwnedSpaceUpdate,
    ) -> Result<AdvancedOwnedSpace, StoreError> {
        let chain = self
            .load_space_chain(update.space_id)?
            .ok_or(StoreError::SpaceNotFound)?;
        let reference = self
            .connection
            .query_row(
                "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                [update.space_id.as_bytes().as_slice()],
                |row| row.get::<_, Option<String>>(0),
            )?
            .map(|value| KeyReference::parse(&value))
            .transpose()?
            .ok_or(StoreError::SpaceAuthorityUnavailable)?;
        let protected = self.key_store.read(KeyKind::SpaceAuthority, &reference)?;
        let secret = SpaceAuthoritySecret::try_from_bytes(protected.as_bytes())?;
        if secret.public_key() != chain.genesis().authority() {
            return Err(StoreError::SchemaMismatch {
                detail: "stored Space authority key does not match genesis",
            });
        }
        let generation =
            chain
                .latest_generation()
                .checked_add(1)
                .ok_or(StoreError::SchemaMismatch {
                    detail: "Space manifest generation exhausted",
                })?;
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(update.space_id, generation, chain.latest_hash()),
            update.issued_at_ms,
            update.membership.clone(),
        )?
        .sign(&secret)?;
        let mut candidate = chain;
        candidate.apply(&manifest)?;
        let persisted = self.persist_space_chain_with_authority(&candidate, Some(&reference))?;
        if let Some(error) = persisted.error() {
            return Err(StoreError::Manifest(error));
        }
        let revision = persisted.revision().map_or_else(|| self.revision(), Ok)?;
        let authorization = SpaceAuthorizationView::from_chain(&candidate);
        Ok(AdvancedOwnedSpace {
            revision,
            chain: candidate,
            authorization,
        })
    }
}
