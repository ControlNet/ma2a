//! Agent names and Agent task identifiers.
//!
//! An [`AgentId`] names one Agent inside the Endpoint that hosts it. It attributes and
//! routes; it never authenticates and never authorizes.

use core::fmt;

use crate::{ProtocolError, space_codec::Decoder};

use super::process_id::parse_hex16;

/// Longest Agent name in bytes.
pub const MAX_AGENT_ID_LEN: usize = 63;

/// The operator-chosen name of one Agent, unique within its hosting Endpoint.
///
/// 1–63 bytes of `[a-z0-9-]`, starting with a letter and not ending with `-`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(String);

impl AgentId {
    /// Validates one Agent name.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for any name outside the closed form.
    pub fn new(name: &str) -> Result<Self, ProtocolError> {
        let bytes = name.as_bytes();
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_AGENT_ID_LEN
            && bytes.first().is_some_and(u8::is_ascii_lowercase)
            && bytes.last() != Some(&b'-')
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        if valid {
            Ok(Self(name.to_owned()))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Self::new(decoder.text(MAX_AGENT_ID_LEN)?)
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A cryptographically random 128-bit Agent task identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentTaskId([u8; 16]);

impl AgentTaskId {
    /// Generates a fresh identifier from the operating system random source.
    ///
    /// # Errors
    /// Returns `ProtocolError::INTERNAL` if the operating system random source fails.
    pub fn random() -> Result<Self, ProtocolError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| ProtocolError::INTERNAL)?;
        Ok(Self(bytes))
    }

    /// Wraps exact identifier bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the exact identifier bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses the 32-character lowercase hexadecimal form.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for any other text.
    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        parse_hex16(text).map(Self)
    }

    pub(crate) fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Self::try_from(decoder.bytes(16)?)
    }
}

impl TryFrom<&[u8]> for AgentTaskId {
    type Error = ProtocolError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <&[u8; 16]>::try_from(value).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(*bytes))
    }
}

impl fmt::Display for AgentTaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}
