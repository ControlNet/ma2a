//! Files v1 terminal results.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    FileMetadata, MAX_LIST_PAGE, MAX_PATH_COMPONENT_LEN, ServiceError,
    cbor::{self, Writer},
};

/// One directory entry name and its metadata.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Entry name, one valid path component.
    pub name: String,
    /// Entry metadata, observed without following a symlink.
    pub metadata: FileMetadata,
}

impl DirEntry {
    /// Creates one entry.
    pub const fn new(name: String, metadata: FileMetadata) -> Self {
        Self { name, metadata }
    }
}

/// Successful terminal result of one Files operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Files results are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesResult {
    /// Stat metadata.
    Metadata(FileMetadata),
    /// One sorted listing page and the cursor of the next page, if any.
    Listing(Vec<DirEntry>, Option<String>),
    /// A read returned this many bytes in its preceding `data` frame; `true` at end of file.
    Read(u32, bool),
    /// A write committed a file of this size.
    Written(u64),
    /// Mkdir, rename, or remove completed.
    Done,
}

/// Terminal Files outcome.
pub type FilesOutcome = Result<FilesResult, ServiceError>;

impl FilesResult {
    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the outcome exceeds its bounds.
    pub fn encode_outcome(outcome: &FilesOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(outcome.as_ref().map_err(|error| *error), Self::encode)
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<FilesOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, Self::decode)
    }

    fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Metadata(metadata) => {
                writer.array(2)?.uint(0);
                metadata.encode(writer)?;
            }
            Self::Listing(entries, cursor) => {
                if entries.len() > MAX_LIST_PAGE {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                writer.array(3)?.uint(1).array(entries.len())?;
                for entry in entries {
                    writer.array(2)?.text(&entry.name)?;
                    entry.metadata.encode(writer)?;
                }
                writer.optional_text(cursor.as_deref())?;
            }
            Self::Read(length, eof) => {
                writer
                    .array(3)?
                    .uint(2)
                    .uint(u64::from(*length))
                    .boolean(*eof);
            }
            Self::Written(size) => {
                writer.array(2)?.uint(3).uint(*size);
            }
            Self::Done => {
                writer.array(1)?.uint(4);
            }
        }
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let length = decoder.array(3)?;
        Ok(match (length, decoder.uint()?) {
            (2, 0) => Self::Metadata(FileMetadata::decode(decoder)?),
            (3, 1) => {
                let count = decoder.array(MAX_LIST_PAGE)?;
                let entries = (0..count)
                    .map(|_| {
                        decoder.array_exact(2)?;
                        let name = decoder.text(MAX_PATH_COMPONENT_LEN)?.to_owned();
                        Ok(DirEntry::new(name, FileMetadata::decode(decoder)?))
                    })
                    .collect::<Result<Vec<_>, ProtocolError>>()?;
                Self::Listing(
                    entries,
                    cbor::optional_text(decoder, MAX_PATH_COMPONENT_LEN)?,
                )
            }
            (3, 2) => Self::Read(cbor::u32_value(decoder)?, decoder.boolean()?),
            (2, 3) => Self::Written(decoder.uint()?),
            (1, 4) => Self::Done,
            _ => return Err(ProtocolError::INVALID_INPUT),
        })
    }
}
