//! Space projection, including the signed member set the owner already holds.

use ma2a_core::SpaceId;

use crate::api::ApiError;

/// Bounded Space summary without membership authorization diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceView {
    pub(crate) id: SpaceId,
    pub(crate) name: String,
    pub(crate) member_count: u32,
}

impl SpaceView {
    /// Creates a bounded Space summary.
    ///
    /// # Errors
    /// Returns invalid input when the Space name length is outside the bound.
    pub fn new(id: SpaceId, name: &str, member_count: u32) -> Result<Self, ApiError> {
        if name.is_empty() || name.len() > 128 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                id,
                name: name.to_owned(),
                member_count,
            })
        }
    }
}
