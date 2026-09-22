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

/// Stable identity of one Space, without any membership state.
///
/// A departure result uses this rather than [`SpaceView`]: after leaving, the
/// Endpoint holds no authoritative membership for that Space, so reporting the
/// pre-departure member count would present stale state as current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceIdentityView {
    pub(crate) id: SpaceId,
    pub(crate) name: String,
}

impl SpaceIdentityView {
    /// Creates a bounded Space identity.
    ///
    /// # Errors
    /// Returns invalid input when the Space name length is outside the bound.
    pub fn new(id: SpaceId, name: &str) -> Result<Self, ApiError> {
        if name.is_empty() || name.len() > 128 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                id,
                name: name.to_owned(),
            })
        }
    }
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
