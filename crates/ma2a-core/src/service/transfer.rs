//! Transfer v1 contracts: typed resources, resumable downloads, verified uploads.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    RemotePath, ServiceAccess, ServiceError, WriteMode,
    cbor::{self, Writer},
};

/// A typed content source or sink. Phase 2 defines files; later phases add resource kinds.
#[expect(
    clippy::exhaustive_enums,
    reason = "transfer resources are a closed wire vocabulary per protocol version"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferResource {
    /// A regular file below an exported root.
    File(RemotePath),
}

impl TransferResource {
    fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::File(path) => writer.array(2)?.uint(0).path(path)?,
        };
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(2)?;
        match decoder.uint()? {
            0 => Ok(Self::File(cbor::path(decoder)?)),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// Observable identity of source content used to prove a resumed download continues it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentIdentity {
    size: u64,
    modified_ns: u64,
}

impl ContentIdentity {
    /// Creates an identity from the size and modification time of the source.
    pub const fn new(size: u64, modified_ns: u64) -> Self {
        Self { size, modified_ns }
    }

    /// Returns the content size in bytes.
    pub const fn size(self) -> u64 {
        self.size
    }

    /// Returns the modification time in Unix nanoseconds.
    pub const fn modified_ns(self) -> u64 {
        self.modified_ns
    }

    pub(crate) fn encode(self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(2)?.uint(self.size).uint(self.modified_ns);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(2)?;
        Ok(Self::new(decoder.uint()?, decoder.uint()?))
    }
}

/// One Transfer stream request.
#[expect(
    clippy::exhaustive_enums,
    reason = "transfer directions are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferRequest {
    /// Download from `offset`; a resumed download names the identity it continues.
    Get(TransferResource, u64, Option<ContentIdentity>),
    /// Upload exactly `size` bytes, finalized atomically with `mode` after verification.
    Put(TransferResource, u64, WriteMode),
}

impl TransferRequest {
    /// Returns the complete access this transfer needs.
    pub fn access(&self) -> ServiceAccess {
        match self {
            Self::Get(TransferResource::File(path), ..) => {
                ServiceAccess::FilesRead(path.root().clone())
            }
            Self::Put(TransferResource::File(path), ..) => {
                ServiceAccess::FilesWrite(path.root().clone())
            }
        }
    }

    /// Returns whether this transfer can change target state.
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Put(..))
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Get(resource, offset, identity) => {
                writer.array(4)?.uint(0);
                resource.encode(writer)?;
                writer.uint(*offset);
                match identity {
                    Some(identity) => identity.encode(writer)?,
                    None => {
                        writer.null();
                    }
                }
            }
            Self::Put(resource, size, mode) => {
                writer.array(4)?.uint(1);
                resource.encode(writer)?;
                writer.uint(*size).uint(mode.code());
            }
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        match decoder.uint()? {
            0 => {
                let resource = TransferResource::decode(decoder)?;
                let offset = decoder.uint()?;
                let identity = if decoder.null() {
                    None
                } else {
                    Some(ContentIdentity::decode(decoder)?)
                };
                if offset > 0 && identity.is_none() {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Ok(Self::Get(resource, offset, identity))
            }
            1 => Ok(Self::Put(
                TransferResource::decode(decoder)?,
                decoder.uint()?,
                WriteMode::from_code(decoder.uint()?)?,
            )),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// Server admission of one transfer.
#[expect(
    clippy::exhaustive_enums,
    reason = "transfer directions are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferAccept {
    /// The download will stream `size - offset` bytes of content with this identity.
    Get(ContentIdentity),
    /// The staged upload is ready to receive data.
    Put,
}

impl TransferAccept {
    /// Encodes the `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] only on internal bound failure.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        match self {
            Self::Get(identity) => {
                writer.array(2)?.uint(0);
                identity.encode(&mut writer)?;
            }
            Self::Put => {
                writer.array(1)?.uint(1);
            }
        }
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }

    /// Decodes an `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let length = decoder.array(2)?;
        let accept = match (length, decoder.uint()?) {
            (2, 0) => Self::Get(ContentIdentity::decode(&mut decoder)?),
            (1, 1) => Self::Put,
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        decoder.finish()?;
        Ok(accept)
    }
}

/// Client commitment sent after the final upload `end`: the BLAKE3 hash of all content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferCommit([u8; 32]);

impl TransferCommit {
    /// Creates a commitment to the BLAKE3 hash of all uploaded content.
    pub const fn new(hash: [u8; 32]) -> Self {
        Self(hash)
    }

    /// Returns the committed BLAKE3 hash.
    pub const fn hash(self) -> [u8; 32] {
        self.0
    }

    /// Encodes the `control` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        writer.array(2)?.uint(0).bytes(&self.0)?;
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }

    /// Decodes a `control` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        decoder.array_exact(2)?;
        decoder.key(0)?;
        let hash = cbor::hash(&mut decoder)?;
        decoder.finish()?;
        Ok(Self(hash))
    }
}

/// Successful terminal transfer summary.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferResult {
    /// Content bytes carried by this stream.
    pub bytes: u64,
    /// Full content size.
    pub size: u64,
    /// BLAKE3 hash of the full content.
    pub hash: [u8; 32],
}

/// Terminal transfer outcome.
pub type TransferOutcome = Result<TransferResult, ServiceError>;

impl TransferResult {
    /// Creates a summary.
    pub const fn new(bytes: u64, size: u64, hash: [u8; 32]) -> Self {
        Self { bytes, size, hash }
    }

    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode_outcome(outcome: &TransferOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(
            outcome.as_ref().map_err(|error| *error),
            |result, writer| {
                writer
                    .array(3)?
                    .uint(result.bytes)
                    .uint(result.size)
                    .bytes(&result.hash)?;
                Ok(())
            },
        )
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<TransferOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, |decoder| {
            decoder.array_exact(3)?;
            Ok(Self::new(
                decoder.uint()?,
                decoder.uint()?,
                cbor::hash(decoder)?,
            ))
        })
    }
}
