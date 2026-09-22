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
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the fixed member wire fields"
    )]
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
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the fixed Space summary wire fields"
    )]
    pub fn new(
        id: SpaceId,
        name: &str,
        head: SpaceChainHead,
        member_count: u32,
    ) -> Result<Self, ApiError> {
        if name.is_empty() || name.len() > 128 || member_count as usize > MAX_SPACE_MEMBERS {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            id,
            name: name.to_owned(),
            member_count,
            generation: head.generation,
            chain_hash: head.chain_hash,
            revoked_count: head.revoked_count,
        })
    }

    /// Returns the verified Space identifier.
    pub const fn space_id(&self) -> SpaceId {
        self.id
    }

    pub(crate) const fn id(&self) -> SpaceId {
        self.id
    }

    /// Returns the shared Space name every member projects identically.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Narrows to the command-result shape, which carries no chain head.
    ///
    /// # Errors
    /// Returns invalid input when the name no longer fits the result bound.
    pub fn to_space_view(&self) -> Result<super::SpaceView, ApiError> {
        super::SpaceView::new(self.id, &self.name, self.member_count)
    }
}

/// Complete members and the summary from the same durable revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceDetailsView {
    pub(crate) revision: u64,
    pub(crate) space: SnapshotSpaceView,
    pub(crate) members: Vec<SpaceMemberView>,
}

impl SpaceDetailsView {
    /// Constructs complete, bounded Space details.
    ///
    /// # Errors
    /// Rejects incomplete membership or duplicate/unsorted member identities.
    pub fn new(
        revision: u64,
        space: SnapshotSpaceView,
        members: Vec<SpaceMemberView>,
    ) -> Result<Self, ApiError> {
        if members.len() != space.member_count as usize
            || members
                .iter()
                .zip(members.iter().skip(1))
                .any(|(left, right)| left.endpoint_id >= right.endpoint_id)
        {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            revision,
            space,
            members,
        })
    }
}

/// Lightweight durable revision and process identity for event polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotStampView {
    pub(crate) revision: u64,
    pub(crate) runtime_boot_id: [u8; 16],
}

impl SnapshotStampView {
    /// Creates a revision stamp for this Runtime boot.
    pub const fn new(revision: u64, runtime_boot_id: [u8; 16]) -> Self {
        Self {
            revision,
            runtime_boot_id,
        }
    }
}
