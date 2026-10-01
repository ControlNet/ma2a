//! Service versions and the interaction class of each operation.

use crate::{ProtocolError, space_codec::Decoder};

use super::{MAX_VERSION_OPERATIONS, OperationName, cbor::Writer};

/// How one operation's frames flow after `open`.
///
/// Descriptive: the Runtime enforces the Services v1 frame rules for every interaction alike.
#[expect(
    clippy::exhaustive_enums,
    reason = "interaction classes are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Interaction {
    /// One request, one terminal outcome.
    Unary,
    /// Structured `data` frames from the service until the outcome.
    Stream,
    /// Raw bytes in either direction, input on channel 0 and output on channel 1.
    Bytes,
    /// Creates or manages a resource that outlives the operation.
    Resource,
}

impl Interaction {
    /// Returns the stable wire code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Unary => 0,
            Self::Stream => 1,
            Self::Bytes => 2,
            Self::Resource => 3,
        }
    }

    /// Decodes a stable wire code.
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::Unary),
            1 => Some(Self::Stream),
            2 => Some(Self::Bytes),
            3 => Some(Self::Resource),
            _ => None,
        }
    }

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unary => "unary",
            Self::Stream => "stream",
            Self::Bytes => "bytes",
            Self::Resource => "resource",
        }
    }

    /// Parses the stable lowercase name.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Unary, Self::Stream, Self::Bytes, Self::Resource]
            .into_iter()
            .find(|interaction| interaction.name() == name)
    }
}

/// One protocol version of a service and the operations it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceVersion {
    version: u16,
    operations: Vec<(OperationName, Interaction)>,
}

impl ServiceVersion {
    /// Validates one version: a non-zero number and 1–16 distinct operations.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] outside those bounds.
    pub fn new(
        version: u16,
        operations: Vec<(OperationName, Interaction)>,
    ) -> Result<Self, ProtocolError> {
        let mut names: Vec<&OperationName> = operations.iter().map(|(name, _)| name).collect();
        names.sort();
        names.dedup();
        if version == 0
            || operations.is_empty()
            || operations.len() > MAX_VERSION_OPERATIONS
            || names.len() != operations.len()
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            version,
            operations,
        })
    }

    /// Returns the version number.
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// Returns the operations in declaration order.
    pub fn operations(&self) -> &[(OperationName, Interaction)] {
        &self.operations
    }

    /// Returns the interaction of one operation, if this version offers it.
    pub fn operation(&self, name: &str) -> Option<Interaction> {
        self.operations
            .iter()
            .find(|(operation, _)| operation.as_str() == name)
            .map(|(_, interaction)| *interaction)
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(2)?
            .uint(u64::from(self.version))
            .array(self.operations.len())?;
        for (name, interaction) in &self.operations {
            writer
                .array(2)?
                .text(name.as_str())?
                .uint(u64::from(interaction.code()));
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(2)?;
        let version = super::cbor::u16_value(decoder)?;
        let count = decoder.array(MAX_VERSION_OPERATIONS)?;
        let mut operations = Vec::with_capacity(count);
        for _ in 0..count {
            decoder.array_exact(2)?;
            let name = OperationName::decode_from(decoder)?;
            let interaction =
                Interaction::from_code(decoder.uint()?).ok_or(ProtocolError::INVALID_INPUT)?;
            operations.push((name, interaction));
        }
        Self::new(version, operations)
    }
}
