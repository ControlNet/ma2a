//! Content-free summary of one served operation, used for audit.

use ma2a_core::service::ServiceError;

/// How one served operation ended.
#[expect(
    clippy::exhaustive_enums,
    reason = "the three terminal classes mirror the reject/outcome frame contract"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationEnd {
    /// A `reject` frame was sent; no side effect happened.
    Rejected(ServiceError),
    /// A successful `outcome` was sent.
    Completed,
    /// The operation started and ended with a failed `outcome` or stream loss.
    Failed(ServiceError),
}

/// Byte counters and terminal class of one operation; never carries paths or content.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationReport {
    /// Terminal class.
    pub end: OperationEnd,
    /// Content bytes received from the caller.
    pub bytes_in: u64,
    /// Content bytes sent to the caller.
    pub bytes_out: u64,
}

impl OperationReport {
    /// Creates a report with zero byte counters.
    pub const fn new(end: OperationEnd) -> Self {
        Self {
            end,
            bytes_in: 0,
            bytes_out: 0,
        }
    }

    /// Returns this report with byte counters.
    #[must_use]
    pub const fn with_bytes(mut self, bytes_in: u64, bytes_out: u64) -> Self {
        self.bytes_in = bytes_in;
        self.bytes_out = bytes_out;
        self
    }
}

/// Converts a `usize` length into a byte counter without truncation.
pub(crate) fn len64(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

/// Lowercase hexadecimal form of bytes, used only for generated local file names.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _written = write!(text, "{byte:02x}");
            text
        })
}
