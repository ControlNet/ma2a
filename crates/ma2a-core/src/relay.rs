use iroh_base::{RelayUrl, SecretKey, Signature};

use crate::space::domain_hash;
use crate::space_codec::{
    Decoder, buffer, write_array, write_bytes, write_map, write_text, write_uint,
};
use crate::{EndpointId, ProtocolError, SpaceId};

/// Domain separator for version 1 private relay advertisement signatures.
pub const PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN: &[u8] =
    b"ma2a-private-relay-advertisement-signature-v1";
/// Domain separator for version 1 signed private relay advertisement hashes.
pub const PRIVATE_RELAY_ADVERTISEMENT_HASH_DOMAIN: &[u8] =
    b"ma2a-private-relay-advertisement-hash-v1";
/// Maximum accepted canonical signed private relay advertisement size.
pub const MAX_PRIVATE_RELAY_ADVERTISEMENT_LEN: usize = 4_096;
/// Maximum UTF-8 byte length of one advertised relay URL.
pub const MAX_PRIVATE_RELAY_URL_LEN: usize = 2_048;
/// Maximum lifetime of one private relay advertisement.
pub const MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS: u64 = 600_000;

/// Monotonic sequence and bounded validity window for one private relay advertisement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivateRelayAdvertisementValidity {
    sequence: u64,
    issued_at_ms: u64,
    expires_at_ms: u64,
}

/// Exact Space and provider Endpoint bound by one private relay advertisement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivateRelayAdvertisementScope {
    space_id: SpaceId,
    provider_endpoint_id: EndpointId,
}

impl PrivateRelayAdvertisementScope {
    /// Creates one exact Space-local provider scope.
    pub const fn new(space_id: SpaceId, provider_endpoint_id: EndpointId) -> Self {
        Self {
            space_id,
            provider_endpoint_id,
        }
    }
}

impl PrivateRelayAdvertisementValidity {
    /// Creates a sequence and validity window of at most ten minutes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an invalid or overlong window.
    pub const fn new(
        sequence: u64,
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<Self, ProtocolError> {
        if expires_at_ms <= issued_at_ms
            || expires_at_ms - issued_at_ms > MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            sequence,
            issued_at_ms,
            expires_at_ms,
        })
    }
}

/// Unsigned Endpoint-owned private relay advertisement for one exact Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateRelayAdvertisementV1 {
    space_id: SpaceId,
    provider_endpoint_id: EndpointId,
    relay_url: RelayUrl,
    validity: PrivateRelayAdvertisementValidity,
}

impl PrivateRelayAdvertisementV1 {
    /// Creates a bounded private relay advertisement.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an unsupported or oversized relay URL.
    pub fn new(
        scope: PrivateRelayAdvertisementScope,
        relay_url: RelayUrl,
        validity: PrivateRelayAdvertisementValidity,
    ) -> Result<Self, ProtocolError> {
        let relay_url_text = relay_url.as_str();
        if relay_url_text.len() > MAX_PRIVATE_RELAY_URL_LEN || relay_url.scheme() != "https" {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            space_id: scope.space_id,
            provider_endpoint_id: scope.provider_endpoint_id,
            relay_url,
            validity,
        })
    }

    /// Signs this advertisement with its declared provider Endpoint identity.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the signer is not the provider.
    pub fn sign(
        self,
        secret: &SecretKey,
    ) -> Result<SignedPrivateRelayAdvertisementV1, ProtocolError> {
        if EndpointId::from(secret.public()) != self.provider_endpoint_id {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let body = self.body_bytes()?;
        let signature = secret.sign(&signature_message(&body)).to_bytes();
        SignedPrivateRelayAdvertisementV1::from_parts(self, signature)
    }

    /// Returns the exact advertised Space.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns the hosting Endpoint identity.
    pub const fn provider_endpoint_id(&self) -> EndpointId {
        self.provider_endpoint_id
    }

    /// Returns the advertised relay URL.
    pub const fn relay_url(&self) -> &RelayUrl {
        &self.relay_url
    }

    /// Returns the provider-owned monotonic sequence.
    pub const fn sequence(&self) -> u64 {
        self.validity.sequence
    }

    /// Returns the issue time in Unix milliseconds.
    pub const fn issued_at_ms(&self) -> u64 {
        self.validity.issued_at_ms
    }

    /// Returns the expiry time in Unix milliseconds.
    pub const fn expires_at_ms(&self) -> u64 {
        self.validity.expires_at_ms
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
        write_bytes(&mut output, self.provider_endpoint_id.as_bytes())?;
        write_uint(&mut output, 3);
        write_text(&mut output, self.relay_url.as_str())?;
        write_uint(&mut output, 4);
        write_uint(&mut output, self.validity.sequence);
        write_uint(&mut output, 5);
        write_uint(&mut output, self.validity.issued_at_ms);
        write_uint(&mut output, 6);
        write_uint(&mut output, self.validity.expires_at_ms);
        Ok(output)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        decoder.map(7)?;
        decoder.key(0)?;
        if decoder.array(2)? != 2 || decoder.uint()? != 1 || decoder.uint()? != 0 {
            return Err(ProtocolError::VERSION_MISMATCH);
        }
        decoder.key(1)?;
        let space_id = SpaceId::try_from(decoder.bytes(32)?)?;
        decoder.key(2)?;
        let provider_endpoint_id = EndpointId::try_from(decoder.bytes(32)?)?;
        decoder.key(3)?;
        let relay_url = decoder
            .text(MAX_PRIVATE_RELAY_URL_LEN)?
            .parse()
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.key(4)?;
        let sequence = decoder.uint()?;
        decoder.key(5)?;
        let issued_at_ms = decoder.uint()?;
        decoder.key(6)?;
        let expires_at_ms = decoder.uint()?;
        decoder.finish()?;
        Self::new(
            PrivateRelayAdvertisementScope::new(space_id, provider_endpoint_id),
            relay_url,
            PrivateRelayAdvertisementValidity::new(sequence, issued_at_ms, expires_at_ms)?,
        )
    }
}

/// Canonically encoded and provider-signed private relay advertisement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedPrivateRelayAdvertisementV1 {
    advertisement: PrivateRelayAdvertisementV1,
    signature: [u8; 64],
    canonical_body: Vec<u8>,
    canonical_bytes: Vec<u8>,
}

impl SignedPrivateRelayAdvertisementV1 {
    /// Parses exact canonical bytes without verifying the signature.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed, non-canonical, or oversized bytes.
    pub fn parse_canonical_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_PRIVATE_RELAY_ADVERTISEMENT_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.map(2)?;
        decoder.key(0)?;
        let body = decoder.bytes(MAX_PRIVATE_RELAY_ADVERTISEMENT_LEN)?;
        decoder.key(1)?;
        let signature =
            *<&[u8; 64]>::try_from(decoder.bytes(64)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.finish()?;
        let signed = Self::from_parts(PrivateRelayAdvertisementV1::decode(body)?, signature)?;
        if signed.canonical_bytes != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(signed)
    }

    /// Verifies the signature against the declared provider Endpoint identity.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when signature verification fails.
    pub fn verify_signature(&self) -> Result<(), ProtocolError> {
        self.advertisement
            .provider_endpoint_id
            .to_public_key()?
            .verify(
                &signature_message(&self.canonical_body),
                &Signature::from_bytes(&self.signature),
            )
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    /// Returns the decoded advertisement.
    pub const fn advertisement(&self) -> &PrivateRelayAdvertisementV1 {
        &self.advertisement
    }

    /// Returns exact canonical signed bytes.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Returns the domain-separated hash of the signed advertisement.
    pub fn advertisement_hash(&self) -> [u8; 32] {
        domain_hash(
            PRIVATE_RELAY_ADVERTISEMENT_HASH_DOMAIN,
            &self.canonical_bytes,
        )
    }

    fn from_parts(
        advertisement: PrivateRelayAdvertisementV1,
        signature: [u8; 64],
    ) -> Result<Self, ProtocolError> {
        let canonical_body = advertisement.body_bytes()?;
        let mut canonical_bytes = buffer()?;
        write_map(&mut canonical_bytes, 2)?;
        write_uint(&mut canonical_bytes, 0);
        write_bytes(&mut canonical_bytes, &canonical_body)?;
        write_uint(&mut canonical_bytes, 1);
        write_bytes(&mut canonical_bytes, &signature)?;
        if canonical_bytes.len() > MAX_PRIVATE_RELAY_ADVERTISEMENT_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            advertisement,
            signature,
            canonical_body,
            canonical_bytes,
        })
    }
}

fn signature_message(body: &[u8]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN.len() + body.len());
    message.extend_from_slice(PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN);
    message.extend_from_slice(body);
    message
}
