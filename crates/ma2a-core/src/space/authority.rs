//! Ed25519 authority key material for one version 1 Space.

use std::fmt;

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};

use crate::ProtocolError;

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

    pub(super) fn verifying_key(self) -> Result<VerifyingKey, ProtocolError> {
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
