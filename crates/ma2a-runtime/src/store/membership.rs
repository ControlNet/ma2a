use ma2a_core::{EndpointId, SpaceId, SpaceManifestMembership, SpaceRevocationV1};

use crate::{error::RuntimeError, store::StoreBackend};

#[derive(Clone, Copy)]
pub(crate) struct OwnedMemberRevocation {
    pub(crate) space_id: SpaceId,
    pub(crate) endpoint_id: EndpointId,
    pub(crate) issued_at_ms: u64,
    pub(crate) local_endpoint_id: EndpointId,
}

impl StoreBackend {
    pub(super) fn revoke_owned_space_member(
        &mut self,
        request: OwnedMemberRevocation,
    ) -> Result<(u64, std::collections::BTreeSet<SpaceId>), RuntimeError> {
        let chain = self
            .repository
            .load_space_chain(request.space_id)?
            .ok_or(ma2a_store::StoreError::SpaceNotFound)?;
        let member_position = chain
            .members()
            .binary_search_by_key(&request.endpoint_id, ma2a_core::SpaceMemberV1::endpoint_id)
            .map_err(|_| ma2a_store::StoreError::SpaceNotFound)?;
        let mut members = chain.members().to_vec();
        members.remove(member_position);
        let mut revocations = chain.revocations().to_vec();
        revocations.push(SpaceRevocationV1::new(request.endpoint_id));
        revocations.sort_by_key(|revocation| revocation.endpoint_id());
        let advanced = self
            .repository
            .advance_owned_space(&ma2a_store::OwnedSpaceUpdate::new(
                request.space_id,
                request.issued_at_ms,
                SpaceManifestMembership::new(members, revocations),
            ))?;
        Ok((
            advanced.revision(),
            self.repository.memberships_for(request.local_endpoint_id)?,
        ))
    }
}
