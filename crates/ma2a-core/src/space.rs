use ed25519_dalek::Signature;

#[path = "space/authority.rs"]
mod authority;

pub use authority::{SpaceAuthorityPublicKey, SpaceAuthoritySecret};

use crate::policy::SpaceMemberV1;
use crate::space_codec::{
    Decoder, MAX_SPACE_OBJECT_LEN, buffer, write_array, write_bytes, write_map, write_text,
    write_uint,
};
use crate::{ProtocolError, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceId, SpacePolicyV1};

/// Domain separator for version 1 genesis signatures.
pub const GENESIS_SIGNATURE_DOMAIN: &[u8] = b"ma2a-space-genesis-signature-v1";
/// Domain separator for the genesis link in a Space chain.
pub const GENESIS_CHAIN_HASH_DOMAIN: &[u8] = b"ma2a-space-genesis-chain-hash-v1";
/// Maximum UTF-8 byte length of the shared Space name carried by genesis.
pub const MAX_SPACE_NAME_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Unsigned version 1 Space genesis body.
pub struct SpaceGenesisV1 {
    nonce: [u8; 32],
    created_at_ms: u64,
    authority: SpaceAuthorityPublicKey,
    initial_member: SpaceMemberV1,
    policy: SpacePolicyV1,
    name: Option<String>,
}

impl SpaceGenesisV1 {
    /// Creates a validated Space genesis body.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the nonce is all zeroes.
    pub fn new(identity: SpaceGenesisIdentity, owner: SpaceGenesisOwner) -> Self {
        Self {
            nonce: identity.nonce,
            created_at_ms: identity.created_at_ms,
            authority: identity.authority,
            initial_member: owner.initial_member,
            policy: owner.policy,
            name: None,
        }
    }

    /// Binds the shared human-readable Space name into this genesis body.
    ///
    /// The name is signed with the rest of genesis and therefore also enters the
    /// derived [`SpaceId`], so every enrolled member reads the same value.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an empty, oversized, or control-bearing name.
    pub fn with_name(mut self, name: &str) -> Result<Self, ProtocolError> {
        self.name = Some(validate_name(name)?.to_owned());
        Ok(self)
    }

    /// Creates a genesis body with a fresh operating-system random nonce.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] when secure randomness is unavailable.
    pub fn create(
        created_at_ms: u64,
        authority: SpaceAuthorityPublicKey,
        owner: SpaceGenesisOwner,
    ) -> Result<Self, ProtocolError> {
        Ok(Self::new(
            SpaceGenesisIdentity::create(created_at_ms, authority)?,
            owner,
        ))
    }

    /// Signs this genesis with its declared authority key.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the signing key is not the declared authority.
    pub fn sign(
        self,
        secret: &SpaceAuthoritySecret,
    ) -> Result<SignedSpaceGenesisV1, ProtocolError> {
        if secret.public_key() != self.authority {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let body = self.body_bytes()?;
        let signature = secret.sign(GENESIS_SIGNATURE_DOMAIN, &body);
        SignedSpaceGenesisV1::from_parts(self, signature)
    }

    /// Returns the declared authority public key.
    pub const fn authority(&self) -> SpaceAuthorityPublicKey {
        self.authority
    }
    /// Returns the initial Space member.
    pub const fn initial_member(&self) -> &SpaceMemberV1 {
        &self.initial_member
    }
    /// Returns the initial Space policy.
    pub const fn policy(&self) -> SpacePolicyV1 {
        self.policy
    }
    /// Returns the shared Space name, absent only for pre-name legacy genesis bodies.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn body_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = buffer()?;
        // A named Space appends exactly one entry, so an unnamed legacy body still
        // encodes byte-for-byte as it did before Space names existed.
        write_map(&mut output, if self.name.is_some() { 7 } else { 6 })?;
        write_uint(&mut output, 0);
        write_array(&mut output, 2)?;
        write_uint(&mut output, 1);
        write_uint(&mut output, 0);
        write_uint(&mut output, 1);
        write_bytes(&mut output, &self.nonce)?;
        write_uint(&mut output, 2);
        write_uint(&mut output, self.created_at_ms);
        write_uint(&mut output, 3);
        write_bytes(&mut output, self.authority.as_bytes())?;
        write_uint(&mut output, 4);
        self.initial_member.encode(&mut output)?;
        write_uint(&mut output, 5);
        self.policy.encode(&mut output)?;
        if let Some(name) = &self.name {
            write_uint(&mut output, 6);
            write_text(&mut output, name)?;
        }
        Ok(output)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let entries = decoder.map_one_of(&[6, 7])?;
        decoder.key(0)?;
        if decoder.array(2)? != 2 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        if decoder.uint()? != 1 || decoder.uint()? != 0 {
            return Err(ProtocolError::VERSION_MISMATCH);
        }
        decoder.key(1)?;
        let nonce =
            *<&[u8; 32]>::try_from(decoder.bytes(32)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.key(2)?;
        let created_at_ms = decoder.uint()?;
        decoder.key(3)?;
        let authority = SpaceAuthorityPublicKey::try_from_bytes(decoder.bytes(32)?)?;
        decoder.key(4)?;
        let initial_member = SpaceMemberV1::decode(&mut decoder)?;
        decoder.key(5)?;
        let policy = SpacePolicyV1::decode(&mut decoder)?;
        let name = if entries == 7 {
            decoder.key(6)?;
            Some(validate_name(decoder.text(MAX_SPACE_NAME_LEN)?)?.to_owned())
        } else {
            None
        };
        decoder.finish()?;
        let genesis = Self::new(
            SpaceGenesisIdentity::new(nonce, created_at_ms, authority)?,
            SpaceGenesisOwner::new(initial_member, policy),
        );
        Ok(match name {
            Some(name) => genesis.with_name(&name)?,
            None => genesis,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Canonically encoded and authority-signed version 1 Space genesis.
pub struct SignedSpaceGenesisV1 {
    genesis: SpaceGenesisV1,
    signature: [u8; 64],
    canonical_body: Vec<u8>,
    canonical_bytes: Vec<u8>,
}

impl SignedSpaceGenesisV1 {
    /// Parses canonical genesis bytes and verifies the embedded authority signature.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed, non-canonical, or invalidly signed bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
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
        let genesis = SpaceGenesisV1::decode(body)?;
        let signed = Self::from_parts(genesis, signature)?;
        if signed.canonical_bytes != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        signed.verify()?;
        Ok(signed)
    }

    /// Returns the decoded genesis body.
    pub const fn genesis(&self) -> &SpaceGenesisV1 {
        &self.genesis
    }
    /// Returns the Space authority public key.
    pub const fn authority(&self) -> SpaceAuthorityPublicKey {
        self.genesis.authority
    }
    /// Returns the exact canonical signed bytes.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    /// Returns the exact canonical unsigned genesis body bytes.
    pub fn canonical_body_bytes(&self) -> &[u8] {
        &self.canonical_body
    }
    /// Returns the Ed25519 signature bytes.
    pub const fn signature_bytes(&self) -> [u8; 64] {
        self.signature
    }
    /// Derives the Space identifier from the canonical unsigned genesis body.
    pub fn space_id(&self) -> SpaceId {
        SpaceId::derive(&self.canonical_body)
    }
    /// Returns the shared Space name, absent only for pre-name legacy genesis bodies.
    pub fn name(&self) -> Option<&str> {
        self.genesis.name()
    }
    /// Returns the genesis link hash used by the first manifest.
    pub fn chain_hash(&self) -> [u8; 32] {
        domain_hash(GENESIS_CHAIN_HASH_DOMAIN, &self.canonical_bytes)
    }

    fn from_parts(genesis: SpaceGenesisV1, signature: [u8; 64]) -> Result<Self, ProtocolError> {
        if genesis.authority.as_bytes() == genesis.initial_member.endpoint_id().as_bytes() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let body = genesis.body_bytes()?;
        let mut canonical_bytes = buffer()?;
        write_map(&mut canonical_bytes, 2)?;
        write_uint(&mut canonical_bytes, 0);
        write_bytes(&mut canonical_bytes, &body)?;
        write_uint(&mut canonical_bytes, 1);
        write_bytes(&mut canonical_bytes, &signature)?;
        Ok(Self {
            genesis,
            signature,
            canonical_body: body,
            canonical_bytes,
        })
    }

    fn verify(&self) -> Result<(), ProtocolError> {
        let body = self.genesis.body_bytes()?;
        let mut message = Vec::with_capacity(GENESIS_SIGNATURE_DOMAIN.len() + body.len());
        message.extend_from_slice(GENESIS_SIGNATURE_DOMAIN);
        message.extend_from_slice(&body);
        self.authority()
            .verifying_key()?
            .verify_strict(&message, &Signature::from_bytes(&self.signature))
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }
}

fn validate_name(name: &str) -> Result<&str, ProtocolError> {
    if name.is_empty() || name.len() > MAX_SPACE_NAME_LEN || name.chars().any(char::is_control) {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(name)
}

pub(crate) fn domain_hash(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}
