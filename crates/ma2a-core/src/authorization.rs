use crate::{Capability, EndpointId, SpaceChain, SpaceId, SpaceMemberV1, SpacePolicyV1};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Immutable authorization state derived from one verified Space chain.
pub struct SpaceAuthorizationView {
    space_id: SpaceId,
    generation: u64,
    manifest_hash: [u8; 32],
    policy: SpacePolicyV1,
    members: Vec<SpaceMemberV1>,
}

impl SpaceAuthorizationView {
    /// Builds an authorization view from the latest state of `chain`.
    pub fn from_chain(chain: &SpaceChain) -> Self {
        Self {
            space_id: chain.space_id(),
            generation: chain.latest_generation(),
            manifest_hash: chain.latest_hash(),
            policy: chain.genesis().genesis().policy(),
            members: chain.members().to_vec(),
        }
    }

    /// Returns whether the endpoint has the capability under both Space policy and membership.
    pub fn allows(&self, endpoint_id: EndpointId, capability: Capability) -> bool {
        self.policy.allows(capability)
            && self
                .members
                .binary_search_by_key(&endpoint_id, SpaceMemberV1::endpoint_id)
                .is_ok_and(|index| {
                    self.members
                        .get(index)
                        .is_some_and(|member| member.capabilities().allows(capability))
                })
    }

    /// Returns whether the Endpoint is a current member of this exact Space.
    pub fn contains_member(&self, endpoint_id: EndpointId) -> bool {
        self.members
            .binary_search_by_key(&endpoint_id, SpaceMemberV1::endpoint_id)
            .is_ok()
    }

    /// Returns the Space identifier represented by this view.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }
    /// Returns the manifest generation represented by this view.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Returns the latest verified chain hash represented by this view.
    pub const fn manifest_hash(&self) -> [u8; 32] {
        self.manifest_hash
    }
}

/// Returns whether any Space grants the endpoint the requested capability.
pub fn authorize_any(
    spaces: &[SpaceAuthorizationView],
    endpoint_id: EndpointId,
    capability: Capability,
) -> bool {
    spaces
        .iter()
        .any(|space| space.allows(endpoint_id, capability))
}
