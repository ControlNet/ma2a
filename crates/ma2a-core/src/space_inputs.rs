use crate::{ProtocolError, SpaceAuthorityPublicKey, SpaceMemberV1, SpacePolicyV1};

/// Immutable identity fields of one Space genesis body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceGenesisIdentity {
    pub(crate) nonce: [u8; 32],
    pub(crate) created_at_ms: u64,
    pub(crate) authority: SpaceAuthorityPublicKey,
}

impl SpaceGenesisIdentity {
    /// Creates deterministic genesis identity fields.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the nonce is all zeroes.
    pub fn new(
        nonce: [u8; 32],
        created_at_ms: u64,
        authority: SpaceAuthorityPublicKey,
    ) -> Result<Self, ProtocolError> {
        if nonce == [0; 32] {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            nonce,
            created_at_ms,
            authority,
        })
    }

    pub(crate) fn create(
        created_at_ms: u64,
        authority: SpaceAuthorityPublicKey,
    ) -> Result<Self, ProtocolError> {
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| ProtocolError::INTERNAL)?;
        Self::new(nonce, created_at_ms, authority)
    }
}

/// Initial membership and policy of one Space genesis body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceGenesisOwner {
    pub(crate) initial_member: SpaceMemberV1,
    pub(crate) policy: SpacePolicyV1,
}

impl SpaceGenesisOwner {
    /// Creates the initial owner state.
    pub const fn new(initial_member: SpaceMemberV1, policy: SpacePolicyV1) -> Self {
        Self {
            initial_member,
            policy,
        }
    }
}
