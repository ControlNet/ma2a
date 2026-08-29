use iroh_base::{SecretKey, Signature, TransportAddr};

use crate::space::domain_hash;
use crate::space_codec::{Decoder, buffer, write_array, write_bytes, write_map, write_uint};
use crate::{EndpointId, ProtocolError, SpaceId};

/// Domain separator for version 1 Endpoint address signatures.
pub const ADDRESS_SIGNATURE_DOMAIN: &[u8] = b"ma2a-space-address-signature-v1";
/// Domain separator for version 1 signed address record hashes.
pub const ADDRESS_RECORD_HASH_DOMAIN: &[u8] = b"ma2a-space-address-record-hash-v1";
/// Maximum accepted canonical signed address record size.
pub const MAX_ADDRESS_RECORD_LEN: usize = 16_384;
/// Maximum number of transport addresses in one record.
pub const MAX_ADDRESS_RECORD_ADDRESSES: usize = 16;
/// Maximum lifetime of one address record.
pub const MAX_ADDRESS_RECORD_VALIDITY_MS: u64 = 600_000;

/// Bounded identity-free Iroh endpoint addressing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressEndpointDataV1 {
    pub(crate) addresses: Vec<TransportAddr>,
}

impl AddressEndpointDataV1 {
    /// Builds canonical endpoint data from relay and IP transport addresses.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for empty, duplicate, custom, or oversized data.
    pub fn new(mut addresses: Vec<TransportAddr>) -> Result<Self, ProtocolError> {
        if addresses.is_empty() || addresses.len() > MAX_ADDRESS_RECORD_ADDRESSES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        addresses.sort_unstable();
        if addresses
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left == right))
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut relays = 0usize;
        for address in &addresses {
            match address {
                TransportAddr::Relay(url) => {
                    relays += 1;
                    if url.to_string().len() > address_codec::MAX_RELAY_URL_LEN {
                        return Err(ProtocolError::INVALID_INPUT);
                    }
                }
                TransportAddr::Ip(socket) if socket.port() != 0 => {}
                TransportAddr::Ip(_) | TransportAddr::Custom(_) => {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                _ => return Err(ProtocolError::INVALID_INPUT),
            }
        }
        if relays > 1 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { addresses })
    }

    /// Returns canonical relay and IP transport addresses.
    pub fn addresses(&self) -> &[TransportAddr] {
        &self.addresses
    }
}

/// Space and Endpoint identity bound by one address record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRecordScope {
    space_id: SpaceId,
    endpoint_id: EndpointId,
}

impl AddressRecordScope {
    /// Creates an exact Space-local Endpoint scope.
    pub const fn new(space_id: SpaceId, endpoint_id: EndpointId) -> Self {
        Self {
            space_id,
            endpoint_id,
        }
    }
}

/// Monotonic sequence and bounded validity window for one address record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRecordValidity {
    sequence: u64,
    issued_at_ms: u64,
    expires_at_ms: u64,
}

impl AddressRecordValidity {
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
            || expires_at_ms - issued_at_ms > MAX_ADDRESS_RECORD_VALIDITY_MS
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

/// Unsigned version 1 Endpoint-owned Space address record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceAddressRecordV1 {
    scope: AddressRecordScope,
    validity: AddressRecordValidity,
    endpoint_data: AddressEndpointDataV1,
}

impl SpaceAddressRecordV1 {
    /// Creates an address record from validated scope, validity, and endpoint data.
    pub const fn new(
        scope: AddressRecordScope,
        validity: AddressRecordValidity,
        endpoint_data: AddressEndpointDataV1,
    ) -> Self {
        Self {
            scope,
            validity,
            endpoint_data,
        }
    }

    /// Signs this record with its declared Endpoint identity.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the signer does not own the record identity.
    pub fn sign(self, secret: &SecretKey) -> Result<SignedSpaceAddressRecordV1, ProtocolError> {
        if EndpointId::from(secret.public()) != self.scope.endpoint_id {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let body = self.body_bytes()?;
        let signature = secret.sign(&signature_message(&body)).to_bytes();
        SignedSpaceAddressRecordV1::from_parts(self, signature)
    }

    /// Returns the Space scope.
    pub const fn space_id(&self) -> SpaceId {
        self.scope.space_id
    }
    /// Returns the Endpoint that owns this address data.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.scope.endpoint_id
    }
    /// Returns the monotonic Endpoint-owned sequence.
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
    /// Returns identity-free endpoint addressing data.
    pub const fn endpoint_data(&self) -> &AddressEndpointDataV1 {
        &self.endpoint_data
    }

    fn body_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = buffer()?;
        write_map(&mut output, 7)?;
        write_uint(&mut output, 0);
        write_array(&mut output, 2)?;
        write_uint(&mut output, 1);
        write_uint(&mut output, 0);
        write_uint(&mut output, 1);
        write_bytes(&mut output, self.scope.space_id.as_bytes())?;
        write_uint(&mut output, 2);
        write_bytes(&mut output, self.scope.endpoint_id.as_bytes())?;
        write_uint(&mut output, 3);
        write_uint(&mut output, self.validity.sequence);
        write_uint(&mut output, 4);
        write_uint(&mut output, self.validity.issued_at_ms);
        write_uint(&mut output, 5);
        write_uint(&mut output, self.validity.expires_at_ms);
        write_uint(&mut output, 6);
        self.endpoint_data.encode(&mut output)?;
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
        let endpoint_id = EndpointId::try_from(decoder.bytes(32)?)?;
        decoder.key(3)?;
        let sequence = decoder.uint()?;
        decoder.key(4)?;
        let issued_at_ms = decoder.uint()?;
        decoder.key(5)?;
        let expires_at_ms = decoder.uint()?;
        decoder.key(6)?;
        let endpoint_data = AddressEndpointDataV1::decode(&mut decoder)?;
        decoder.finish()?;
        let scope = AddressRecordScope::new(space_id, endpoint_id);
        let validity = AddressRecordValidity::new(sequence, issued_at_ms, expires_at_ms)?;
        Ok(Self::new(scope, validity, endpoint_data))
    }
}

/// Canonically encoded and Endpoint-signed version 1 Space address record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedSpaceAddressRecordV1 {
    record: SpaceAddressRecordV1,
    signature: [u8; 64],
    canonical_body: Vec<u8>,
    canonical_bytes: Vec<u8>,
}

impl SignedSpaceAddressRecordV1 {
    /// Parses exact canonical bytes without verifying the signature.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed, non-canonical, or oversized bytes.
    pub fn parse_canonical_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_ADDRESS_RECORD_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.map(2)?;
        decoder.key(0)?;
        let body = decoder.bytes(MAX_ADDRESS_RECORD_LEN)?;
        decoder.key(1)?;
        let signature =
            *<&[u8; 64]>::try_from(decoder.bytes(64)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.finish()?;
        let signed = Self::from_parts(SpaceAddressRecordV1::decode(body)?, signature)?;
        if signed.canonical_bytes != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(signed)
    }

    /// Verifies the Endpoint signature bound to the outer record identity.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when signature verification fails.
    pub fn verify_signature(&self) -> Result<(), ProtocolError> {
        self.record
            .scope
            .endpoint_id
            .to_public_key()?
            .verify(
                &signature_message(&self.canonical_body),
                &Signature::from_bytes(&self.signature),
            )
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    /// Returns the decoded address record.
    pub const fn record(&self) -> &SpaceAddressRecordV1 {
        &self.record
    }
    /// Returns exact canonical signed bytes.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    /// Returns exact canonical unsigned body bytes.
    pub fn canonical_body_bytes(&self) -> &[u8] {
        &self.canonical_body
    }
    /// Returns the domain-separated hash of the signed record.
    pub fn record_hash(&self) -> [u8; 32] {
        domain_hash(ADDRESS_RECORD_HASH_DOMAIN, &self.canonical_bytes)
    }

    fn from_parts(
        record: SpaceAddressRecordV1,
        signature: [u8; 64],
    ) -> Result<Self, ProtocolError> {
        let canonical_body = record.body_bytes()?;
        let mut canonical_bytes = buffer()?;
        write_map(&mut canonical_bytes, 2)?;
        write_uint(&mut canonical_bytes, 0);
        write_bytes(&mut canonical_bytes, &canonical_body)?;
        write_uint(&mut canonical_bytes, 1);
        write_bytes(&mut canonical_bytes, &signature)?;
        if canonical_bytes.len() > MAX_ADDRESS_RECORD_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            record,
            signature,
            canonical_body,
            canonical_bytes,
        })
    }
}

fn signature_message(body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(ADDRESS_SIGNATURE_DOMAIN.len() + body.len());
    message.extend_from_slice(ADDRESS_SIGNATURE_DOMAIN);
    message.extend_from_slice(body);
    message
}

mod address_codec;
