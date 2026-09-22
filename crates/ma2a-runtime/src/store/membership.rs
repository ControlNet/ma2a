use std::collections::BTreeSet;

use ma2a_core::{EndpointId, SpaceChain, SpaceId, SpaceManifestMembership, SpaceRevocationV1};

use crate::{error::RuntimeError, store::StoreBackend};

/// Outcome of one owner-signed member removal, including the signed chain the
/// removed member needs in order to converge.
pub(crate) struct RemovedMember {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<SpaceId>,
    pub(crate) chain: SpaceChain,
}

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
    ) -> Result<RemovedMember, RuntimeError> {
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
        Ok(RemovedMember {
            revision: advanced.revision(),
            memberships: self.repository.memberships_for(request.local_endpoint_id)?,
            chain: advanced.chain().clone(),
        })
    }

    pub(super) fn advance_owned_space(
        &mut self,
        update: &ma2a_store::OwnedSpaceUpdate,
        local_endpoint_id: EndpointId,
    ) -> Result<(u64, BTreeSet<SpaceId>), RuntimeError> {
        let advanced = self.repository.advance_owned_space(update)?;
        Ok((
            advanced.revision(),
            self.repository.memberships_for(local_endpoint_id)?,
        ))
    }

    /// Reads the Space memberships the Store currently holds for this Endpoint.
    pub(super) fn memberships(
        &self,
        local_endpoint_id: EndpointId,
    ) -> Result<BTreeSet<SpaceId>, RuntimeError> {
        self.repository
            .memberships_for(local_endpoint_id)
            .map_err(Into::into)
    }

    pub(super) fn load_space_chain(
        &self,
        space_id: SpaceId,
    ) -> Result<Option<SpaceChain>, RuntimeError> {
        self.repository
            .load_space_chain(space_id)
            .map_err(Into::into)
    }

    /// Persists the authority-signed chain that removed the local Endpoint.
    pub(super) fn persist_departure(
        &mut self,
        chain: &SpaceChain,
        local_endpoint_id: EndpointId,
    ) -> Result<(u64, BTreeSet<SpaceId>), RuntimeError> {
        let persisted = self.repository.persist_space_chain(chain)?;
        if let Some(error) = persisted.error() {
            return Err(ma2a_store::StoreError::Manifest(error).into());
        }
        let revision = persisted
            .revision()
            .map_or_else(|| self.repository.revision(), Ok)?;
        Ok((
            revision,
            self.repository.memberships_for(local_endpoint_id)?,
        ))
    }
}
