//! Service stream framing v1: `type:u8 || length:u32 BE || payload`.

use crate::ProtocolError;

/// Byte length of one frame header.
pub const FRAME_HEADER_LEN: usize = 5;
/// Maximum payload of `open`, `accept`, and `control` frames.
pub const MAX_HEADER_FRAME_LEN: usize = 16_384;
/// Maximum payload of an `outcome` frame.
pub const MAX_OUTCOME_FRAME_LEN: usize = 65_536;
/// Maximum content bytes carried by one `data` frame, excluding its channel byte.
pub const MAX_DATA_CHUNK_LEN: usize = 65_536;

/// Logical channel of a `data` or `end` frame.
#[expect(
    clippy::exhaustive_enums,
    reason = "service channels are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Request body, stdin, terminal input, or proxy bytes toward the target.
    Input,
    /// Response body, stdout, terminal output, or proxy bytes from the target.
    Output,
    /// Standard error of an Exec child.
    Error,
}

impl Channel {
    /// Returns the stable wire code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Input => 0,
            Self::Output => 1,
            Self::Error => 2,
        }
    }

    /// Decodes a stable wire code.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an unknown channel.
    pub const fn from_code(code: u8) -> Result<Self, ProtocolError> {
        match code {
            0 => Ok(Self::Input),
            1 => Ok(Self::Output),
            2 => Ok(Self::Error),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// The closed set of service frame types.
#[expect(
    clippy::exhaustive_enums,
    reason = "service frame types are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// First client frame: canonical request header.
    Open,
    /// Server admission of the request after complete authorization.
    Accept,
    /// Server refusal carrying one status code; guarantees no side effect happened.
    Reject,
    /// Channel-tagged content bytes.
    Data,
    /// Logical half-close of one channel.
    End,
    /// Canonical in-band control message.
    Control,
    /// Terminal summary written by the server.
    Outcome,
    /// Client request to stop the operation.
    Cancel,
}

impl FrameKind {
    const fn code(self) -> u8 {
        match self {
            Self::Open => 1,
            Self::Accept => 2,
            Self::Reject => 3,
            Self::Data => 4,
            Self::End => 5,
            Self::Control => 6,
            Self::Outcome => 7,
            Self::Cancel => 8,
        }
    }

    const fn from_code(code: u8) -> Result<Self, ProtocolError> {
        match code {
            1 => Ok(Self::Open),
            2 => Ok(Self::Accept),
            3 => Ok(Self::Reject),
            4 => Ok(Self::Data),
            5 => Ok(Self::End),
            6 => Ok(Self::Control),
            7 => Ok(Self::Outcome),
            8 => Ok(Self::Cancel),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }

    /// Returns the inclusive payload length bounds of this frame type.
    pub const fn payload_bounds(self) -> (usize, usize) {
        match self {
            Self::Open | Self::Accept | Self::Control => (1, MAX_HEADER_FRAME_LEN),
            Self::Outcome => (1, MAX_OUTCOME_FRAME_LEN),
            Self::Reject | Self::End => (1, 1),
            Self::Data => (2, MAX_DATA_CHUNK_LEN + 1),
            Self::Cancel => (0, 0),
        }
    }
}

/// One validated frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    kind: FrameKind,
    length: usize,
}

impl FrameHeader {
    /// Creates a header after checking the payload bound of `kind`.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when `length` is outside the bound.
    pub const fn new(kind: FrameKind, length: usize) -> Result<Self, ProtocolError> {
        let (minimum, maximum) = kind.payload_bounds();
        if length < minimum || length > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { kind, length })
    }

    /// Parses and bounds one received header before any payload byte is read.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an unknown type or out-of-bound length.
    pub fn parse(bytes: [u8; FRAME_HEADER_LEN]) -> Result<Self, ProtocolError> {
        let [kind, length @ ..] = bytes;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| ProtocolError::INVALID_INPUT)?;
        Self::new(FrameKind::from_code(kind)?, length)
    }

    /// Encodes the exact header bytes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] if the length cannot be represented.
    pub fn encode(self) -> Result<[u8; FRAME_HEADER_LEN], ProtocolError> {
        let length = u32::try_from(self.length).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let [a, b, c, d] = length.to_be_bytes();
        Ok([self.kind.code(), a, b, c, d])
    }

    /// Returns the frame type.
    pub const fn kind(self) -> FrameKind {
        self.kind
    }

    /// Returns the payload length.
    pub const fn length(self) -> usize {
        self.length
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameHeader, FrameKind, MAX_DATA_CHUNK_LEN, MAX_HEADER_FRAME_LEN};

    #[test]
    fn headers_roundtrip_and_bounds_are_enforced_before_payloads() {
        let header = FrameHeader::new(FrameKind::Data, MAX_DATA_CHUNK_LEN + 1).expect("bound");
        let parsed = FrameHeader::parse(header.encode().expect("encode")).expect("parse");
        assert_eq!(parsed, header);
        assert!(FrameHeader::new(FrameKind::Open, MAX_HEADER_FRAME_LEN + 1).is_err());
        assert!(FrameHeader::new(FrameKind::Cancel, 1).is_err());
        assert!(FrameHeader::new(FrameKind::Data, 1).is_err());
        assert!(FrameHeader::parse([9, 0, 0, 0, 1]).is_err());
        assert!(FrameHeader::parse([1, 0xff, 0xff, 0xff, 0xff]).is_err());
    }
}
