use crate::{SpaceId, SpaceMemberV1, SpaceRevocationV1};

/// Link fields that place one manifest in a Space chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceManifestLink {
    pub(crate) space_id: SpaceId,
    pub(crate) generation: u64,
    pub(crate) previous_hash: [u8; 32],
}

impl SpaceManifestLink {
    /// Creates one manifest chain link.
    pub const fn new(space_id: SpaceId, generation: u64, previous_hash: [u8; 32]) -> Self {
        Self {
            space_id,
            generation,
            previous_hash,
        }
    }
}

/// Complete membership and revocation state carried by one manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceManifestMembership {
    pub(crate) members: Vec<SpaceMemberV1>,
    pub(crate) revocations: Vec<SpaceRevocationV1>,
}

impl SpaceManifestMembership {
    /// Creates one complete manifest membership state.
    pub const fn new(members: Vec<SpaceMemberV1>, revocations: Vec<SpaceRevocationV1>) -> Self {
        Self {
            members,
            revocations,
        }
    }
}
