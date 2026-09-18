//! Bounded control-round history, retained only in memory.

use crate::api::ApiError;

/// The Runtime keeps the most recent rounds only; this is a diagnostic, not a log.
pub const MAX_RETAINED_CONTROL_ROUNDS: usize = 16;

/// How one scheduled control round finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlRoundView {
    pub(crate) at_ms: u64,
    pub(crate) peer_count: u32,
    pub(crate) outcome: &'static str,
}

impl ControlRoundView {
    /// Creates one completed-round record.
    ///
    /// # Errors
    /// Returns invalid input for an outcome outside the closed set.
    pub fn new(at_ms: u64, peer_count: u32, outcome: &'static str) -> Result<Self, ApiError> {
        if !matches!(outcome, "succeeded" | "failed" | "empty") {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            at_ms,
            peer_count,
            outcome,
        })
    }
}
