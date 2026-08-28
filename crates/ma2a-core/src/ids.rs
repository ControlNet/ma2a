//! Fixed-width identifiers used by the Phase 1 ontology and wire contract.

use iroh_base::PublicKey;

use crate::ProtocolError;

const SPACE_DOMAIN: &[u8] = b"ma2a-space-v1";

/// The validated 32-byte Iroh public key of one Runtime endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId([u8; PublicKey::LENGTH]);

impl EndpointId {
    /// Returns the exact public-key bytes carried on the wire.
    pub const fn as_bytes(&self) -> &[u8; PublicKey::LENGTH] {
        &self.0
    }

    /// Converts the identifier back into the validated Iroh public key.
    ///
    /// # Errors
    /// Returns `ProtocolError::INVALID_INPUT` if the stored bytes fail Iroh validation.
    pub fn to_public_key(self) -> Result<PublicKey, ProtocolError> {
        PublicKey::from_bytes(&self.0).map_err(|_| ProtocolError::INVALID_INPUT)
    }
}

impl From<PublicKey> for EndpointId {
    fn from(value: PublicKey) -> Self {
        Self(*value.as_bytes())
    }
}

impl TryFrom<&[u8]> for EndpointId {
    type Error = ProtocolError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <&[u8; PublicKey::LENGTH]>::try_from(value)
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        PublicKey::from_bytes(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(*bytes))
    }
}

/// The domain-separated hash of exact canonical Space genesis bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId([u8; 32]);

impl SpaceId {
    /// Derives an identifier from exact canonical genesis CBOR bytes.
    pub fn derive(canonical_genesis_cbor: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SPACE_DOMAIN);
        hasher.update(canonical_genesis_cbor);
        Self(*hasher.finalize().as_bytes())
    }

    /// Returns the exact hash bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl TryFrom<&[u8]> for SpaceId {
    type Error = ProtocolError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <&[u8; 32]>::try_from(value).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(*bytes))
    }
}

/// A cryptographically random 128-bit request correlation identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId([u8; 16]);

impl RequestId {
    /// Generates a fresh identifier from the operating system random source.
    ///
    /// # Errors
    /// Returns `ProtocolError::INTERNAL` if the operating system random source fails.
    pub fn random() -> Result<Self, ProtocolError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| ProtocolError::INTERNAL)?;
        Ok(Self(bytes))
    }

    /// Returns the exact request identifier bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl TryFrom<&[u8]> for RequestId {
    type Error = ProtocolError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <&[u8; 16]>::try_from(value).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(*bytes))
    }
}
