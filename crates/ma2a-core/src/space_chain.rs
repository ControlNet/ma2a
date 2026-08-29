use std::{error::Error, fmt};

use crate::manifest::endpoint_set;
use crate::space_codec::{
    Decoder, MAX_SPACE_CHAIN_LEN, MAX_SPACE_CHAIN_MANIFESTS, MAX_SPACE_OBJECT_LEN, buffer,
    write_array, write_bytes, write_map, write_uint,
};
use crate::{
    EndpointId, ProtocolError, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceId, SpaceMemberV1,
    SpaceRevocationV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManifestErrorKind {
    InvalidEncoding,
    InvalidSignature,
    Rollback,
    Fork,
    SkippedGeneration,
    InvalidRevocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Stable rejection reason for a Space manifest or exported chain.
pub struct ManifestError(ManifestErrorKind);

impl ManifestError {
    /// The public chain encoding is malformed or non-canonical.
    pub const INVALID_ENCODING: Self = Self(ManifestErrorKind::InvalidEncoding);
    /// A genesis or manifest signature is invalid.
    pub const INVALID_SIGNATURE: Self = Self(ManifestErrorKind::InvalidSignature);
    /// A proposal attempts to replace verified history with an older prefix.
    pub const ROLLBACK: Self = Self(ManifestErrorKind::Rollback);
    /// A proposal diverges from verified Space history.
    pub const FORK: Self = Self(ManifestErrorKind::Fork);
    /// A proposal does not advance by exactly one generation.
    pub const SKIPPED_GENERATION: Self = Self(ManifestErrorKind::SkippedGeneration);
    /// A membership removal is missing its matching Space-local revocation.
    pub const INVALID_REVOCATION: Self = Self(ManifestErrorKind::InvalidRevocation);
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            ManifestErrorKind::InvalidEncoding => "invalid Space chain encoding",
            ManifestErrorKind::InvalidSignature => "invalid Space authority signature",
            ManifestErrorKind::Rollback => "Space manifest rollback rejected",
            ManifestErrorKind::Fork => "Space manifest fork rejected",
            ManifestErrorKind::SkippedGeneration => "Space manifest generation was skipped",
            ManifestErrorKind::InvalidRevocation => "invalid Space revocation transition",
        })
    }
}

impl Error for ManifestError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplyOutcomeKind {
    Advanced,
    Idempotent,
}

/// Outcome of applying a valid signed manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestApplyOutcome(ApplyOutcomeKind);

impl ManifestApplyOutcome {
    /// The chain advanced to the manifest generation.
    pub const ADVANCED: Self = Self(ApplyOutcomeKind::Advanced);
    /// The manifest exactly matched the already-applied generation.
    pub const IDEMPOTENT: Self = Self(ApplyOutcomeKind::Idempotent);
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Verified genesis-to-latest Space manifest chain and derived membership state.
pub struct SpaceChain {
    genesis: SignedSpaceGenesisV1,
    manifests: Vec<SignedSpaceManifestV1>,
    members: Vec<SpaceMemberV1>,
    revocations: Vec<SpaceRevocationV1>,
    latest_hash: [u8; 32],
}

impl SpaceChain {
    /// Starts a verified chain at signed genesis.
    ///
    /// # Errors
    /// Returns [`ManifestError::INVALID_SIGNATURE`] when genesis verification fails.
    pub fn from_genesis(genesis: SignedSpaceGenesisV1) -> Result<Self, ManifestError> {
        SignedSpaceGenesisV1::from_canonical_bytes(genesis.canonical_bytes())
            .map_err(|_| ManifestError::INVALID_SIGNATURE)?;
        let members = vec![genesis.genesis().initial_member().clone()];
        let latest_hash = genesis.chain_hash();
        Ok(Self {
            genesis,
            manifests: Vec::new(),
            members,
            revocations: Vec::new(),
            latest_hash,
        })
    }

    /// Applies one signed manifest under strict linear-chain transition rules.
    ///
    /// # Errors
    /// Returns [`ManifestError`] for invalid signatures, forks, rollback, skips, or revocations.
    pub fn apply(
        &mut self,
        manifest: &SignedSpaceManifestV1,
    ) -> Result<ManifestApplyOutcome, ManifestError> {
        if manifest.manifest().space_id() != self.space_id() {
            return Err(ManifestError::FORK);
        }
        manifest
            .verify(self.genesis.authority())
            .map_err(|_| ManifestError::INVALID_SIGNATURE)?;
        let generation = manifest.generation();
        let latest_generation = self.latest_generation();
        if generation < latest_generation {
            return Err(ManifestError::ROLLBACK);
        }
        if generation == latest_generation {
            return if manifest.manifest_hash() == self.latest_hash {
                Ok(ManifestApplyOutcome::IDEMPOTENT)
            } else {
                Err(ManifestError::FORK)
            };
        }
        let next = latest_generation
            .checked_add(1)
            .ok_or(ManifestError::SKIPPED_GENERATION)?;
        if generation != next {
            return Err(ManifestError::SKIPPED_GENERATION);
        }
        if manifest.previous_hash() != self.latest_hash {
            return Err(ManifestError::FORK);
        }
        if manifest.members().len() > self.genesis.genesis().policy().maximum_members() {
            return Err(ManifestError::INVALID_REVOCATION);
        }
        validate_transition(
            MembershipState::new(&self.members, &self.revocations),
            MembershipState::new(manifest.members(), manifest.revocations()),
        )?;
        self.members.clone_from(&manifest.members().to_vec());
        self.revocations
            .clone_from(&manifest.revocations().to_vec());
        self.latest_hash = manifest.manifest_hash();
        self.manifests.push(manifest.clone());
        Ok(ManifestApplyOutcome::ADVANCED)
    }

    /// Encodes the signed public chain without authority secret material.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when canonical encoding cannot be allocated or produced.
    pub fn export_public(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.manifests.len() > MAX_SPACE_CHAIN_MANIFESTS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut output = buffer()?;
        write_map(&mut output, 2)?;
        write_uint(&mut output, 0);
        write_bytes(&mut output, self.genesis.canonical_bytes())?;
        write_uint(&mut output, 1);
        write_array(&mut output, self.manifests.len())?;
        for manifest in &self.manifests {
            write_bytes(&mut output, manifest.canonical_bytes())?;
        }
        Ok(output)
    }

    /// Parses and verifies a canonical public Space chain.
    ///
    /// # Errors
    /// Returns [`ManifestError`] when encoding, signatures, or transitions are invalid.
    pub fn import_public(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_SPACE_CHAIN_LEN {
            return Err(ManifestError::INVALID_ENCODING);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.map(2).map_err(invalid_encoding)?;
        decoder.key(0).map_err(invalid_encoding)?;
        let genesis_bytes = decoder
            .bytes(MAX_SPACE_OBJECT_LEN)
            .map_err(invalid_encoding)?;
        let genesis = SignedSpaceGenesisV1::from_canonical_bytes(genesis_bytes)
            .map_err(|_| ManifestError::INVALID_SIGNATURE)?;
        decoder.key(1).map_err(invalid_encoding)?;
        let count = decoder
            .array(MAX_SPACE_CHAIN_MANIFESTS)
            .map_err(invalid_encoding)?;
        let mut manifest_bytes = Vec::new();
        manifest_bytes
            .try_reserve(count)
            .map_err(|_| ManifestError::INVALID_ENCODING)?;
        for _ in 0..count {
            manifest_bytes.push(
                decoder
                    .bytes(MAX_SPACE_OBJECT_LEN)
                    .map_err(invalid_encoding)?
                    .to_vec(),
            );
        }
        decoder.finish().map_err(invalid_encoding)?;
        let mut chain = Self::from_genesis(genesis)?;
        for bytes in manifest_bytes {
            let manifest =
                SignedSpaceManifestV1::from_canonical_bytes(&bytes, chain.genesis.authority())
                    .map_err(|_| ManifestError::INVALID_SIGNATURE)?;
            chain.apply(&manifest)?;
        }
        Ok(chain)
    }

    /// Returns the signed Space genesis.
    pub const fn genesis(&self) -> &SignedSpaceGenesisV1 {
        &self.genesis
    }
    /// Returns applied manifests in generation order.
    pub fn manifests(&self) -> &[SignedSpaceManifestV1] {
        &self.manifests
    }
    /// Returns the latest complete sorted member set.
    pub fn members(&self) -> &[SpaceMemberV1] {
        &self.members
    }
    /// Returns the latest complete sorted Space-local revocation set.
    pub fn revocations(&self) -> &[SpaceRevocationV1] {
        &self.revocations
    }
    /// Returns the Space identifier.
    pub fn space_id(&self) -> SpaceId {
        self.genesis.space_id()
    }
    /// Returns the latest applied generation, or zero at genesis.
    pub fn latest_generation(&self) -> u64 {
        self.manifests
            .last()
            .map_or(0, SignedSpaceManifestV1::generation)
    }
    /// Returns the latest genesis or manifest chain hash.
    pub const fn latest_hash(&self) -> [u8; 32] {
        self.latest_hash
    }
}

#[derive(Clone, Copy)]
struct MembershipState<'a> {
    members: &'a [SpaceMemberV1],
    revocations: &'a [SpaceRevocationV1],
}

impl<'a> MembershipState<'a> {
    const fn new(members: &'a [SpaceMemberV1], revocations: &'a [SpaceRevocationV1]) -> Self {
        Self {
            members,
            revocations,
        }
    }
}

fn validate_transition(
    previous: MembershipState<'_>,
    next: MembershipState<'_>,
) -> Result<(), ManifestError> {
    let previous_member_ids = endpoint_set(previous.members);
    let previous_revoked_ids: Vec<EndpointId> = previous
        .revocations
        .iter()
        .map(|value| value.endpoint_id())
        .collect();
    let next_member_ids = endpoint_set(next.members);
    let next_revoked_ids: Vec<EndpointId> = next
        .revocations
        .iter()
        .map(|value| value.endpoint_id())
        .collect();
    let removals_are_explicit = previous_member_ids
        .into_iter()
        .filter(|id| next_member_ids.binary_search(id).is_err())
        .all(|id| next_revoked_ids.binary_search(&id).is_ok());
    let retained_or_readded = previous_revoked_ids.iter().all(|id| {
        next_revoked_ids.binary_search(id).is_ok() || next_member_ids.binary_search(id).is_ok()
    });
    let revocations_have_history = next_revoked_ids.iter().all(|id| {
        previous_revoked_ids.binary_search(id).is_ok()
            || previous
                .members
                .binary_search_by_key(id, SpaceMemberV1::endpoint_id)
                .is_ok()
    });
    if removals_are_explicit && retained_or_readded && revocations_have_history {
        Ok(())
    } else {
        Err(ManifestError::INVALID_REVOCATION)
    }
}

const fn invalid_encoding(_: ProtocolError) -> ManifestError {
    ManifestError::INVALID_ENCODING
}
