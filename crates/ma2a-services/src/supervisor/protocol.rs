//! Requests a daemon sends one supervisor after the ownership proof, one per connection.
//!
//! The request travels as the payload of an `open` frame; everything after it reuses the
//! Services v1 frames: `accept` and `reject`, `data` for output and stdin, `control` for
//! status changes and skipped output, and `outcome` as the terminal frame.

use ma2a_core::service::{LogFrom, ProcessSignal, ServiceError};

/// One supervisor request.
#[expect(
    clippy::exhaustive_enums,
    reason = "supervisor requests are a closed internal vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorRequest {
    /// Stream every status change as `control`, ending with the final one as `outcome`.
    Watch,
    /// Spawn the child now; repeating it is harmless.
    Go,
    /// Follow output from a position; `true` also takes the stdin writer lease.
    Attach(LogFrom, bool),
    /// Follow output from a position and take the stdin writer lease, detaching if stdin closes.
    AttachStrict(LogFrom),
    /// Write stdin until `end`; `true` then closes the child's stdin for good.
    Stdin(bool),
    /// Send one signal to the child's process group.
    Signal(ProcessSignal),
    /// Terminate the child, then kill it after this grace period in milliseconds.
    Terminate(u32),
    /// Exit without ever spawning, if no child exists yet.
    Abort,
}

impl SupervisorRequest {
    /// Encodes the request.
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(12);
        match self {
            Self::Watch => bytes.push(0),
            Self::Go => bytes.push(1),
            Self::Attach(from, writer) => {
                bytes.push(2);
                push_from(&mut bytes, from);
                bytes.push(u8::from(writer));
            }
            Self::AttachStrict(from) => {
                bytes.push(7);
                push_from(&mut bytes, from);
            }
            Self::Stdin(close) => bytes.extend_from_slice(&[3, u8::from(close)]),
            Self::Signal(signal) => {
                let index = ProcessSignal::ALL
                    .iter()
                    .position(|candidate| *candidate == signal)
                    .and_then(|index| u8::try_from(index).ok())
                    .unwrap_or(u8::MAX);
                bytes.extend_from_slice(&[4, index]);
            }
            Self::Terminate(grace_ms) => {
                bytes.push(5);
                bytes.extend_from_slice(&grace_ms.to_be_bytes());
            }
            Self::Abort => bytes.push(6),
        }
        bytes
    }

    /// Decodes a request written by [`Self::to_bytes`].
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for any other bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ServiceError> {
        let request = match bytes {
            [0] => Self::Watch,
            [1] => Self::Go,
            [2, rest @ ..] => {
                let (from, rest) = pull_from(rest)?;
                match rest {
                    [writer @ (0 | 1)] => Self::Attach(from, *writer == 1),
                    _ => return Err(ServiceError::InvalidInput),
                }
            }
            [3, close @ (0 | 1)] => Self::Stdin(*close == 1),
            [4, index] => Self::Signal(
                *ProcessSignal::ALL
                    .get(usize::from(*index))
                    .ok_or(ServiceError::InvalidInput)?,
            ),
            [5, grace @ ..] => Self::Terminate(u32::from_be_bytes(
                <[u8; 4]>::try_from(grace).map_err(|_| ServiceError::InvalidInput)?,
            )),
            [6] => Self::Abort,
            [7, rest @ ..] => {
                let (from, rest) = pull_from(rest)?;
                if !rest.is_empty() {
                    return Err(ServiceError::InvalidInput);
                }
                Self::AttachStrict(from)
            }
            _ => return Err(ServiceError::InvalidInput),
        };
        Ok(request)
    }
}

fn push_from(bytes: &mut Vec<u8>, from: LogFrom) {
    match from {
        LogFrom::Start => bytes.push(0),
        LogFrom::Now => bytes.push(1),
        LogFrom::Cursor(cursor) => {
            bytes.push(2);
            bytes.extend_from_slice(&cursor.to_be_bytes());
        }
    }
}

fn pull_from(bytes: &[u8]) -> Result<(LogFrom, &[u8]), ServiceError> {
    match bytes {
        [0, rest @ ..] => Ok((LogFrom::Start, rest)),
        [1, rest @ ..] => Ok((LogFrom::Now, rest)),
        [2, rest @ ..] => {
            let (cursor, rest) = rest
                .split_first_chunk::<8>()
                .ok_or(ServiceError::InvalidInput)?;
            Ok((LogFrom::Cursor(u64::from_be_bytes(*cursor)), rest))
        }
        _ => Err(ServiceError::InvalidInput),
    }
}

/// The payload of an `accept` or `outcome` that carries nothing; frames are never empty.
pub const ACKNOWLEDGED: &[u8] = &[0];

/// Encodes the `accept` of an attach: where reading starts, the window, and truncation.
pub fn encode_reading(window: (u64, u64, u64), truncated: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(25);
    for value in <[u64; 3]>::from(window) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.push(u8::from(truncated));
    bytes
}

/// Decodes an attach `accept` written by [`encode_reading`].
///
/// # Errors
/// Returns [`ServiceError::InvalidInput`] for any other bytes.
pub fn decode_reading(bytes: &[u8]) -> Result<((u64, u64, u64), bool), ServiceError> {
    let invalid = ServiceError::InvalidInput;
    let (start, rest) = bytes.split_first_chunk::<8>().ok_or(invalid)?;
    let (first, rest) = rest.split_first_chunk::<8>().ok_or(invalid)?;
    let (end, rest) = rest.split_first_chunk::<8>().ok_or(invalid)?;
    let truncated = match rest {
        [0] => false,
        [1] => true,
        _ => return Err(invalid),
    };
    let values = (
        u64::from_be_bytes(*start),
        u64::from_be_bytes(*first),
        u64::from_be_bytes(*end),
    );
    if values.1 > values.2 || values.0 > values.2 {
        return Err(invalid);
    }
    Ok((values, truncated))
}

/// Encodes the `outcome` of a stdin request: bytes delivered and whether stdin is closed.
pub fn encode_written(bytes: u64, closed: bool) -> Vec<u8> {
    let mut encoded = bytes.to_be_bytes().to_vec();
    encoded.push(u8::from(closed));
    encoded
}

/// Decodes a stdin `outcome` written by [`encode_written`].
///
/// # Errors
/// Returns [`ServiceError::InvalidInput`] for any other bytes.
pub const fn decode_written(bytes: &[u8]) -> Result<(u64, bool), ServiceError> {
    match bytes.split_first_chunk::<8>() {
        Some((count, [0])) => Ok((u64::from_be_bytes(*count), false)),
        Some((count, [1])) => Ok((u64::from_be_bytes(*count), true)),
        _ => Err(ServiceError::InvalidInput),
    }
}
