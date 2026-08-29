use ed25519_dalek::{Signature, VerifyingKey};

use crate::policy::{MAX_SPACE_MEMBERS, encode_members};
use crate::space::{SpaceAuthorityPublicKey, SpaceAuthoritySecret, domain_hash};
use crate::space_codec::{
    Decoder, MAX_SPACE_OBJECT_LEN, buffer, write_array, write_bytes, write_map, write_uint,
};
use crate::{
    EndpointId, ProtocolError, SpaceId, SpaceManifestLink, SpaceManifestMembership, SpaceMemberV1,
    SpaceRevocationV1,
};

/// Domain separator for version 1 manifest signatures.
pub const MANIFEST_SIGNATURE_DOMAIN: &[u8] = b"ma2a-space-manifest-signature-v1";
/// Domain separator for version 1 manifest chain hashes.
pub const MANIFEST_HASH_DOMAIN: &[u8] = b"ma2a-space-manifest-hash-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
/// Unsigned version 1 Space membership manifest.
pub struct SpaceManifestV1 {
    space_id: SpaceId,
    generation: u64,
    previous_hash: [u8; 32],
    issued_at_ms: u64,
    members: Vec<SpaceMemberV1>,
    revocations: Vec<SpaceRevocationV1>,
}

impl SpaceManifestV1 {
    /// Creates a validated manifest body.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when generation, ordering, bounds, or revocations are invalid.
    pub fn new(
        link: SpaceManifestLink,
        issued_at_ms: u64,
        membership: SpaceManifestMembership,
    ) -> Result<Self, ProtocolError> {
        let SpaceManifestMembership {
            members,
            revocations,
        } = membership;
        if link.generation == 0
            || members.is_empty()
            || members.len() > MAX_SPACE_MEMBERS
            || revocations.len() > MAX_SPACE_MEMBERS
            || !strict_members(&members)
            || !strict_revocations(&revocations)
            || overlaps(&members, &revocations)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            space_id: link.space_id,
            generation: link.generation,
            previous_hash: link.previous_hash,
            issued_at_ms,
            members,
            revocations,
        })
    }

    /// Signs this manifest with the Space authority key.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] if canonical encoding cannot be produced.
    pub fn sign(
        self,
        secret: &SpaceAuthoritySecret,
    ) -> Result<SignedSpaceManifestV1, ProtocolError> {
        let body = self.body_bytes()?;
        let signature = secret.sign(MANIFEST_SIGNATURE_DOMAIN, &body);
        SignedSpaceManifestV1::from_parts(self, signature)
    }

    /// Returns the Space identifier.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }
    /// Returns this manifest generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Returns the previous chain hash.
    pub const fn previous_hash(&self) -> [u8; 32] {
        self.previous_hash
    }
    /// Returns the complete sorted member set.
    pub fn members(&self) -> &[SpaceMemberV1] {
        &self.members
    }
    /// Returns revocations applied by this generation.
    pub fn revocations(&self) -> &[SpaceRevocationV1] {
        &self.revocations
    }

    fn body_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = buffer()?;
        write_map(&mut output, 7)?;
        write_uint(&mut output, 0);
        write_array(&mut output, 2)?;
        write_uint(&mut output, 1);
        write_uint(&mut output, 0);
        write_uint(&mut output, 1);
        write_bytes(&mut output, self.space_id.as_bytes())?;
        write_uint(&mut output, 2);
        write_uint(&mut output, self.generation);
        write_uint(&mut output, 3);
        write_bytes(&mut output, &self.previous_hash)?;
        write_uint(&mut output, 4);
        write_uint(&mut output, self.issued_at_ms);
        write_uint(&mut output, 5);
        encode_members(&mut output, &self.members)?;
        write_uint(&mut output, 6);
        write_array(&mut output, self.revocations.len())?;
        for revocation in &self.revocations {
            revocation.encode(&mut output)?;
        }
        Ok(output)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        decoder.map(7)?;
        decoder.key(0)?;
        if decoder.array(2)? != 2 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        if decoder.uint()? != 1 || decoder.uint()? != 0 {
            return Err(ProtocolError::VERSION_MISMATCH);
        }
        decoder.key(1)?;
        let space_id = SpaceId::try_from(decoder.bytes(32)?)?;
        decoder.key(2)?;
        let generation = decoder.uint()?;
        decoder.key(3)?;
        let previous_hash =
            *<&[u8; 32]>::try_from(decoder.bytes(32)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.key(4)?;
        let issued_at_ms = decoder.uint()?;
        decoder.key(5)?;
        let member_count = decoder.array(MAX_SPACE_MEMBERS)?;
        let mut members = Vec::new();
        members
            .try_reserve(member_count)
            .map_err(|_| ProtocolError::INTERNAL)?;
        for _ in 0..member_count {
            members.push(SpaceMemberV1::decode(&mut decoder)?);
        }
        decoder.key(6)?;
        let revocation_count = decoder.array(MAX_SPACE_MEMBERS)?;
        let mut revocations = Vec::new();
        revocations
            .try_reserve(revocation_count)
            .map_err(|_| ProtocolError::INTERNAL)?;
        for _ in 0..revocation_count {
            revocations.push(SpaceRevocationV1::decode(&mut decoder)?);
        }
        decoder.finish()?;
        Self::new(
            SpaceManifestLink::new(space_id, generation, previous_hash),
            issued_at_ms,
            SpaceManifestMembership::new(members, revocations),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Canonically encoded and authority-signed version 1 Space manifest.
pub struct SignedSpaceManifestV1 {
    manifest: SpaceManifestV1,
    signature: [u8; 64],
    canonical_bytes: Vec<u8>,
}

impl SignedSpaceManifestV1 {
    /// Parses canonical bytes and verifies the authority signature.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed, non-canonical, or invalidly signed bytes.
    pub fn from_canonical_bytes(
        bytes: &[u8],
        authority: SpaceAuthorityPublicKey,
    ) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_SPACE_OBJECT_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.map(2)?;
        decoder.key(0)?;
        let body = decoder.bytes(MAX_SPACE_OBJECT_LEN)?;
        decoder.key(1)?;
        let signature =
            *<&[u8; 64]>::try_from(decoder.bytes(64)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.finish()?;
        let signed = Self::from_parts(SpaceManifestV1::decode(body)?, signature)?;
        if signed.canonical_bytes != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        signed.verify(authority)?;
        Ok(signed)
    }

    /// Returns the decoded manifest body.
    pub const fn manifest(&self) -> &SpaceManifestV1 {
        &self.manifest
    }
    /// Returns this manifest generation.
    pub const fn generation(&self) -> u64 {
        self.manifest.generation
    }
    /// Returns the previous chain hash.
    pub const fn previous_hash(&self) -> [u8; 32] {
        self.manifest.previous_hash
    }
    /// Returns the complete sorted member set.
    pub fn members(&self) -> &[SpaceMemberV1] {
        &self.manifest.members
    }
    /// Returns revocations applied by this generation.
    pub fn revocations(&self) -> &[SpaceRevocationV1] {
        &self.manifest.revocations
    }
    /// Returns the exact canonical signed bytes.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    /// Returns the Ed25519 signature bytes.
    pub const fn signature_bytes(&self) -> [u8; 64] {
        self.signature
    }
    /// Returns the domain-separated hash of the canonical signed bytes.
    pub fn manifest_hash(&self) -> [u8; 32] {
        domain_hash(MANIFEST_HASH_DOMAIN, &self.canonical_bytes)
    }

    pub(crate) fn verify(&self, authority: SpaceAuthorityPublicKey) -> Result<(), ProtocolError> {
        let key = VerifyingKey::from_bytes(authority.as_bytes())
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        let body = self.manifest.body_bytes()?;
        let mut message = Vec::with_capacity(MANIFEST_SIGNATURE_DOMAIN.len() + body.len());
        message.extend_from_slice(MANIFEST_SIGNATURE_DOMAIN);
        message.extend_from_slice(&body);
        key.verify_strict(&message, &Signature::from_bytes(&self.signature))
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    fn from_parts(manifest: SpaceManifestV1, signature: [u8; 64]) -> Result<Self, ProtocolError> {
        let body = manifest.body_bytes()?;
        let mut canonical_bytes = buffer()?;
        write_map(&mut canonical_bytes, 2)?;
        write_uint(&mut canonical_bytes, 0);
        write_bytes(&mut canonical_bytes, &body)?;
        write_uint(&mut canonical_bytes, 1);
        write_bytes(&mut canonical_bytes, &signature)?;
        Ok(Self {
            manifest,
            signature,
            canonical_bytes,
        })
    }
}

fn strict_members(members: &[SpaceMemberV1]) -> bool {
    members
        .windows(2)
        .all(|pair| matches!(pair, [left, right] if left.endpoint_id() < right.endpoint_id()))
}

fn strict_revocations(revocations: &[SpaceRevocationV1]) -> bool {
    revocations
        .windows(2)
        .all(|pair| matches!(pair, [left, right] if left.endpoint_id() < right.endpoint_id()))
}

fn overlaps(members: &[SpaceMemberV1], revocations: &[SpaceRevocationV1]) -> bool {
    members.iter().any(|member| {
        revocations
            .binary_search_by_key(&member.endpoint_id(), |item| item.endpoint_id())
            .is_ok()
    })
}

pub(crate) fn endpoint_set(members: &[SpaceMemberV1]) -> Vec<EndpointId> {
    members.iter().map(SpaceMemberV1::endpoint_id).collect()
}
