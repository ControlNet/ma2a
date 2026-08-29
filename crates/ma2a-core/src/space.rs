use std::fmt;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};

use crate::policy::SpaceMemberV1;
use crate::space_codec::{
    Decoder, MAX_SPACE_OBJECT_LEN, buffer, write_array, write_bytes, write_map, write_uint,
};
use crate::{ProtocolError, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceId, SpacePolicyV1};

/// Domain separator for version 1 genesis signatures.
pub const GENESIS_SIGNATURE_DOMAIN: &[u8] = b"ma2a-space-genesis-signature-v1";
/// Domain separator for the genesis link in a Space chain.
pub const GENESIS_CHAIN_HASH_DOMAIN: &[u8] = b"ma2a-space-genesis-chain-hash-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Validated Ed25519 public key for a Space authority.
pub struct SpaceAuthorityPublicKey([u8; 32]);

impl SpaceAuthorityPublicKey {
    /// Parses and validates an Ed25519 authority public key.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an invalid length, point, or weak key.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let bytes = <&[u8; 32]>::try_from(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let key = VerifyingKey::from_bytes(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
        if key.is_weak() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self(*bytes))
    }

    /// Returns the exact public-key bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn verifying_key(self) -> Result<VerifyingKey, ProtocolError> {
        VerifyingKey::from_bytes(&self.0).map_err(|_| ProtocolError::INVALID_INPUT)
    }
}

/// Ed25519 signing key for a Space authority.
pub struct SpaceAuthoritySecret(SigningKey);

impl SpaceAuthoritySecret {
    /// Constructs an authority signing key from a 32-byte seed.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    /// Constructs an authority signing key from a protected 32-byte seed buffer.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when `bytes` is not exactly 32 bytes.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let seed = <&[u8; 32]>::try_from(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(SigningKey::from_bytes(seed)))
    }

    /// Generates an authority signing key from the operating-system random source.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] when secure randomness is unavailable.
    pub fn random() -> Result<Self, ProtocolError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| ProtocolError::INTERNAL)?;
        let key = Self::from_bytes(bytes);
        bytes.fill(0);
        Ok(key)
    }

    /// Returns the matching authority public key.
    pub fn public_key(&self) -> SpaceAuthorityPublicKey {
        SpaceAuthorityPublicKey(self.0.verifying_key().to_bytes())
    }

    /// Returns the 32-byte secret seed for protected storage.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub(crate) fn sign(&self, domain: &[u8], body: &[u8]) -> [u8; 64] {
        let mut message = Vec::with_capacity(domain.len() + body.len());
        message.extend_from_slice(domain);
        message.extend_from_slice(body);
        self.0.sign(&message).to_bytes()
    }
}

impl fmt::Debug for SpaceAuthoritySecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SpaceAuthoritySecret([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Unsigned version 1 Space genesis body.
pub struct SpaceGenesisV1 {
    nonce: [u8; 32],
    created_at_ms: u64,
    authority: SpaceAuthorityPublicKey,
    initial_member: SpaceMemberV1,
    policy: SpacePolicyV1,
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
        }
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

    fn body_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = buffer()?;
        write_map(&mut output, 6)?;
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
        Ok(output)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        decoder.map(6)?;
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
        decoder.finish()?;
        Ok(Self::new(
            SpaceGenesisIdentity::new(nonce, created_at_ms, authority)?,
            SpaceGenesisOwner::new(initial_member, policy),
        ))
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
    /// Returns the genesis link hash used by the first manifest.
    pub fn chain_hash(&self) -> [u8; 32] {
        domain_hash(GENESIS_CHAIN_HASH_DOMAIN, &self.canonical_bytes)
    }

    fn from_parts(genesis: SpaceGenesisV1, signature: [u8; 64]) -> Result<Self, ProtocolError> {
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

pub(crate) fn domain_hash(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}
