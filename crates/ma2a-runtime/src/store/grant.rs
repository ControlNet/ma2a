//! Owner-signed changes of one member's streaming service grants.

use std::collections::BTreeSet;

use ma2a_core::{
    Capability, EndpointId, MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1,
};
use ma2a_store::StoreError;

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    store::StoreBackend,
};

/// Service capabilities a grant sets; every other capability bit is left as it was.
pub(crate) const SERVICE_CAPABILITIES: [Capability; 7] = [
    Capability::FILES_READ,
    Capability::FILES_WRITE,
    Capability::EXEC,
    Capability::SHELL,
    Capability::PROXY,
    Capability::PROCESS,
    Capability::AGENT,
];

/// One requested grant change, applied by the Space owner.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OwnedMemberGrant {
    pub(crate) space_id: SpaceId,
    pub(crate) endpoint_id: EndpointId,
    /// Service grants in [`SERVICE_CAPABILITIES`] order.
    pub(crate) services: [bool; 7],
    pub(crate) issued_at_ms: u64,
    pub(crate) local_endpoint_id: EndpointId,
}

/// Why a grant was refused before anything was signed or committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantRefusal {
    /// The Space or the member is unknown here.
    NotFound,
    /// This Runtime does not hold the Space authority.
    NotOwner,
}

/// Outcome of a grant: the member as now signed and the projection it produced.
#[derive(Debug)]
pub(crate) struct GrantedMember {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<SpaceId>,
    pub(crate) projection_revision: u64,
    pub(crate) member: SpaceMemberV1,
    /// Whether a new manifest generation was signed; an identical grant signs nothing.
    pub(crate) advanced: bool,
}

pub(crate) type GrantResult = Result<Result<GrantedMember, GrantRefusal>, RuntimeError>;

impl StoreBackend {
    pub(super) fn grant_owned_space_member(&mut self, request: OwnedMemberGrant) -> GrantResult {
        let Some(chain) = self.repository.load_space_chain(request.space_id)? else {
            return Ok(Err(GrantRefusal::NotFound));
        };
        let Ok(position) = chain
            .members()
            .binary_search_by_key(&request.endpoint_id, SpaceMemberV1::endpoint_id)
        else {
            return Ok(Err(GrantRefusal::NotFound));
        };
        let mut members = chain.members().to_vec();
        let Some(slot) = members.get_mut(position) else {
            return Ok(Err(GrantRefusal::NotFound));
        };
        let capabilities = granted(slot.capabilities(), request.services);
        if capabilities == slot.capabilities() {
            let member = slot.clone();
            let projection = self
                .repository
                .membership_projection(request.local_endpoint_id)?;
            return Ok(Ok(GrantedMember {
                revision: projection.revision(),
                projection_revision: projection.revision(),
                memberships: projection.into_value(),
                member,
                advanced: false,
            }));
        }
        *slot = SpaceMemberV1::new(slot.endpoint_id(), slot.label().to_owned(), capabilities)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let member = slot.clone();
        let update = ma2a_store::OwnedSpaceUpdate::new(
            request.space_id,
            request.issued_at_ms,
            SpaceManifestMembership::new(members, chain.revocations().to_vec()),
        );
        let advanced = match self.repository.advance_owned_space(&update) {
            Ok(advanced) => advanced,
            Err(StoreError::SpaceAuthorityUnavailable) => return Ok(Err(GrantRefusal::NotOwner)),
            Err(error) => return Err(error.into()),
        };
        let projection = self
            .repository
            .membership_projection(request.local_endpoint_id)
            .map_err(|error| RuntimeError::from(error).after_commit(advanced.revision()))?;
        Ok(Ok(GrantedMember {
            revision: advanced.revision(),
            projection_revision: projection.revision(),
            memberships: projection.into_value(),
            member,
            advanced: true,
        }))
    }
}

/// Replaces exactly the service capability bits of `current`.
pub(crate) fn granted(current: MemberCapabilities, services: [bool; 7]) -> MemberCapabilities {
    SERVICE_CAPABILITIES.into_iter().zip(services).fold(
        current,
        |capabilities, (capability, enabled)| {
            if enabled {
                capabilities.with(capability)
            } else {
                capabilities.without(capability)
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use ma2a_core::{Capability, MemberCapabilities};

    use super::granted;

    #[test]
    fn a_grant_replaces_service_bits_and_keeps_echo_and_relay() {
        let current = MemberCapabilities::new(true, true).with(Capability::SHELL);
        let next = granted(current, [true, false, true, false, false, true, true]);
        assert!(next.allows(Capability::ECHO));
        assert!(next.allows(Capability::PRIVATE_RELAY_PROVIDER));
        assert!(next.allows(Capability::FILES_READ));
        assert!(!next.allows(Capability::FILES_WRITE));
        assert!(next.allows(Capability::EXEC));
        assert!(!next.allows(Capability::SHELL));
        assert!(!next.allows(Capability::PROXY));
        assert!(next.allows(Capability::PROCESS));
        assert!(next.allows(Capability::AGENT));
    }
}
