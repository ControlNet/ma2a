//! The Process resource identifier.

use core::fmt;

use crate::{ProtocolError, space_codec::Decoder};

/// A cryptographically random 128-bit Process resource identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId([u8; 16]);

impl ProcessId {
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
        let digits = text.as_bytes();
        if digits.len() != 32 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            let [high, low] = pair else {
                return Err(ProtocolError::INVALID_INPUT);
            };
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Ok(Self(bytes))
    }

    pub(crate) fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Self::try_from(decoder.bytes(16)?)
    }
}

const fn nibble(digit: u8) -> Result<u8, ProtocolError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(ProtocolError::INVALID_INPUT),
    }
}

impl TryFrom<&[u8]> for ProcessId {
    type Error = ProtocolError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <&[u8; 16]>::try_from(value).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Ok(Self(*bytes))
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}
