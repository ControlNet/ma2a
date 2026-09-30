//! Files v1 request and result contracts.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    RemotePath, ServiceAccess, ServiceError,
    cbor::{self, Writer},
};

/// Maximum content bytes of one small read or write.
pub const MAX_SMALL_FILE_LEN: u32 = 65_536;
/// Maximum entries returned by one directory listing page.
pub const MAX_LIST_PAGE: usize = 128;

/// How a write treats an existing destination.
#[expect(
    clippy::exhaustive_enums,
    reason = "write modes are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Fail with `already_exists` unless the destination is absent (atomic no-clobber).
    CreateNew,
    /// Atomically replace any existing file.
    Replace,
}

impl WriteMode {
    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::CreateNew => 0,
            Self::Replace => 1,
        }
    }

    pub(crate) const fn from_code(code: u64) -> Result<Self, ProtocolError> {
        match code {
            0 => Ok(Self::CreateNew),
            1 => Ok(Self::Replace),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// One Files operation. Write content follows `accept` as one `data` frame.
#[expect(
    clippy::exhaustive_enums,
    reason = "Files operations are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesRequest {
    /// Return metadata without following a final symlink.
    Stat(RemotePath),
    /// Return one sorted directory page after `cursor`.
    List(RemotePath, Option<String>),
    /// Create a directory, optionally with missing parents.
    Mkdir(RemotePath, bool),
    /// Return at most `length` bytes from `offset`.
    Read(RemotePath, u64, u32),
    /// Write exactly `length` bytes supplied after `accept`.
    Write(RemotePath, WriteMode, u32),
    /// Rename within one root; `true` permits replacing the destination.
    Rename(RemotePath, RemotePath, bool),
    /// Remove a file, symlink, or empty directory; `true` removes a tree.
    Remove(RemotePath, bool),
}

impl FilesRequest {
    /// Returns the complete access this operation needs.
    pub fn access(&self) -> ServiceAccess {
        match self {
            Self::Stat(path) | Self::List(path, _) | Self::Read(path, _, _) => {
                ServiceAccess::FilesRead(path.root().clone())
            }
            Self::Mkdir(path, _)
            | Self::Write(path, _, _)
            | Self::Rename(path, _, _)
            | Self::Remove(path, _) => ServiceAccess::FilesWrite(path.root().clone()),
        }
    }

    /// Returns whether this operation can change target state.
    pub const fn is_mutation(&self) -> bool {
        !matches!(self, Self::Stat(_) | Self::List(..) | Self::Read(..))
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Stat(path) => writer.array(2)?.uint(0).path(path)?,
            Self::List(path, cursor) => writer
                .array(3)?
                .uint(1)
                .path(path)?
                .optional_text(cursor.as_deref())?,
            Self::Mkdir(path, parents) => writer.array(3)?.uint(2).path(path)?.boolean(*parents),
            Self::Read(path, offset, length) => writer
                .array(4)?
                .uint(3)
                .path(path)?
                .uint(*offset)
                .uint(u64::from(*length)),
            Self::Write(path, mode, length) => writer
                .array(4)?
                .uint(4)
                .path(path)?
                .uint(mode.code())
                .uint(u64::from(*length)),
            Self::Rename(from, to, replace) => writer
                .array(4)?
                .uint(5)
                .path(from)?
                .path(to)?
                .boolean(*replace),
            Self::Remove(path, recursive) => {
                writer.array(3)?.uint(6).path(path)?.boolean(*recursive)
            }
        };
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let length = decoder.array(4)?;
        let request = match (length, decoder.uint()?) {
            (2, 0) => Self::Stat(cbor::path(decoder)?),
            (3, 1) => Self::List(
                cbor::path(decoder)?,
                cbor::optional_text(decoder, super::MAX_PATH_COMPONENT_LEN)?,
            ),
            (3, 2) => Self::Mkdir(cbor::path(decoder)?, decoder.boolean()?),
            (4, 3) => Self::Read(cbor::path(decoder)?, decoder.uint()?, small(decoder)?),
            (4, 4) => Self::Write(
                cbor::path(decoder)?,
                WriteMode::from_code(decoder.uint()?)?,
                small(decoder)?,
            ),
            (4, 5) => {
                let (from, to) = (cbor::path(decoder)?, cbor::path(decoder)?);
                if from.root() != to.root() || from.is_root() || to.is_root() {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Self::Rename(from, to, decoder.boolean()?)
            }
            (3, 6) => Self::Remove(cbor::path(decoder)?, decoder.boolean()?),
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(request)
    }
}

fn small(decoder: &mut Decoder<'_>) -> Result<u32, ProtocolError> {
    let length = cbor::u32_value(decoder)?;
    if length <= MAX_SMALL_FILE_LEN {
        Ok(length)
    } else {
        Err(ProtocolError::INVALID_INPUT)
    }
}

/// The kind of one filesystem object, observed without following a final symlink.
#[expect(
    clippy::exhaustive_enums,
    reason = "file kinds are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link; its target is never disclosed.
    Symlink,
    /// Any other object, such as a device, socket, or FIFO.
    Other,
}

impl FileKind {
    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

/// Bounded metadata of one filesystem object.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMetadata {
    /// Object kind.
    pub kind: FileKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in Unix milliseconds, when the platform reports one.
    pub modified_ms: Option<u64>,
    /// Whether the platform marks the object read-only for this user.
    pub read_only: bool,
}

impl FileMetadata {
    /// Creates metadata for one object with no modification time and write access.
    pub const fn new(kind: FileKind, size: u64) -> Self {
        Self {
            kind,
            size,
            modified_ms: None,
            read_only: false,
        }
    }

    /// Returns this metadata with a modification time.
    #[must_use]
    pub const fn with_modified_ms(mut self, modified_ms: Option<u64>) -> Self {
        self.modified_ms = modified_ms;
        self
    }

    /// Returns this metadata with a read-only flag.
    #[must_use]
    pub const fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        let kind = match self.kind {
            FileKind::File => 0,
            FileKind::Directory => 1,
            FileKind::Symlink => 2,
            FileKind::Other => 3,
        };
        writer
            .array(4)?
            .uint(kind)
            .uint(self.size)
            .optional_uint(self.modified_ms)
            .boolean(self.read_only);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let kind = match decoder.uint()? {
            0 => FileKind::File,
            1 => FileKind::Directory,
            2 => FileKind::Symlink,
            3 => FileKind::Other,
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(Self::new(kind, decoder.uint()?)
            .with_modified_ms(cbor::optional_uint(decoder)?)
            .with_read_only(decoder.boolean()?))
    }
}

pub use super::files_result::{DirEntry, FilesOutcome, FilesResult};

impl From<ProtocolError> for ServiceError {
    fn from(error: ProtocolError) -> Self {
        Self::from_protocol(error)
    }
}
