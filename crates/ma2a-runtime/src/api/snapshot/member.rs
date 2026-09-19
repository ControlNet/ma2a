//! Signed Space facts the owner's own chain already holds.

use ma2a_core::{EndpointId, MAX_SPACE_MEMBERS, SpaceId};

use crate::api::ApiError;

/// One signed member of a Space, without any authorization diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMemberView {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) label: String,
    pub(crate) echo: bool,
    pub(crate) relay_provider: bool,
}

impl SpaceMemberView {
    /// Creates one bounded member record.
    ///
    /// # Errors
    /// Returns invalid input when the label length is outside the v1 bound.
    pub fn new(
        endpoint_id: EndpointId,
        label: &str,
        echo: bool,
        relay_provider: bool,
    ) -> Result<Self, ApiError> {
        if label.is_empty() || label.len() > 64 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                endpoint_id,
                label: label.to_owned(),
                echo,
                relay_provider,
            })
        }
    }
}

/// A Space as the authoritative snapshot projects it, with its signed chain head.
///
/// This is deliberately distinct from [`super::SpaceView`]: a command result such
/// as `space_created` cannot honestly carry a chain head, so it keeps the smaller
/// shape rather than inventing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSpaceView {
    pub(crate) id: SpaceId,
    pub(crate) name: String,
    pub(crate) member_count: u32,
    pub(crate) generation: u64,
    pub(crate) chain_hash: [u8; 32],
    pub(crate) members: Vec<SpaceMemberView>,
    pub(crate) revoked_count: u32,
}

/// Signed chain head facts grouped for construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceChainHead {
    generation: u64,
    chain_hash: [u8; 32],
    revoked_count: u32,
}

impl SpaceChainHead {
    /// Creates the latest accepted chain head facts.
    pub const fn new(generation: u64, chain_hash: [u8; 32], revoked_count: u32) -> Self {
        Self {
            generation,
            chain_hash,
            revoked_count,
        }
    }
}

impl SnapshotSpaceView {
    /// Creates a bounded Space projection carrying its signed chain head.
    ///
    /// # Errors
    /// Returns invalid input when the name length is outside the bound or the
    /// member set exceeds the v1 maximum.
    pub fn new(
        id: SpaceId,
        name: &str,
        head: SpaceChainHead,
        members: Vec<SpaceMemberView>,
    ) -> Result<Self, ApiError> {
        if name.is_empty() || name.len() > 128 || members.len() > MAX_SPACE_MEMBERS {
            return Err(ApiError::invalid_input());
        }
        let member_count = u32::try_from(members.len()).map_err(|_| ApiError::invalid_input())?;
        Ok(Self {
            id,
            name: name.to_owned(),
            member_count,
            generation: head.generation,
            chain_hash: head.chain_hash,
            members,
            revoked_count: head.revoked_count,
        })
    }

    pub(crate) const fn id(&self) -> SpaceId {
        self.id
    }

    /// Narrows to the command-result shape, which carries no chain head.
    ///
    /// # Errors
    /// Returns invalid input when the name no longer fits the result bound.
    pub fn to_space_view(&self) -> Result<super::SpaceView, ApiError> {
        super::SpaceView::new(self.id, &self.name, self.member_count)
    }
}
