use std::collections::BTreeSet;

use ma2a_core::{
    EndpointId, RequestId, SpaceChain, SpaceId, SpaceManifestMembership, SpaceRevocationV1,
};

use crate::{error::RuntimeError, store::StoreBackend};

/// Outcome of one owner-signed member removal, including the signed chain the
/// removed member needs in order to converge.
#[derive(Debug)]
pub(crate) struct RemovedMember {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<SpaceId>,
    pub(crate) projection_revision: u64,
    pub(crate) chain: SpaceChain,
    /// The departure request was answered before: `chain` is that answer, and nothing new
    /// was committed.
    pub(crate) replayed: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct OwnedMemberRevocation {
    pub(crate) space_id: SpaceId,
    pub(crate) endpoint_id: EndpointId,
    pub(crate) issued_at_ms: u64,
    pub(crate) local_endpoint_id: EndpointId,
    /// The member's own departure request this removal answers, if it answers one.
    pub(crate) request_id: Option<RequestId>,
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
        if request.endpoint_id == chain.genesis().genesis().initial_member().endpoint_id() {
            return Err(ma2a_store::StoreError::SpaceOwnerCannotBeRemoved.into());
        }
        let departure = request
            .request_id
            .map(|request_id| ma2a_store::DepartureRequest::new(request.endpoint_id, request_id));
        if let Some(departure) = departure
            && let Some(generation) = self
                .repository
                .departure_receipt(request.space_id, departure)?
        {
            // A retry of a request already answered, perhaps after the member joined again,
            // gets the same answer and never removes the member a second time.
            let answer = crate::departure::through_generation(&chain, generation);
            return self.replay(request.local_endpoint_id, answer);
        }
        let Ok(member_position) = chain
            .members()
            .binary_search_by_key(&request.endpoint_id, ma2a_core::SpaceMemberV1::endpoint_id)
        else {
            // A departure from before receipts were kept converges on the chain that already
            // removed its sender, and learns nothing signed after that removal.
            let answer = departure
                .and_then(|_| crate::departure::through_removal(&chain, request.endpoint_id));
            return self.replay(request.local_endpoint_id, answer);
        };
        let mut members = chain.members().to_vec();
        members.remove(member_position);
        let mut revocations = chain.revocations().to_vec();
        revocations.push(SpaceRevocationV1::new(request.endpoint_id));
        revocations.sort_by_key(|revocation| revocation.endpoint_id());
        let mut update = ma2a_store::OwnedSpaceUpdate::new(
            request.space_id,
            request.issued_at_ms,
            SpaceManifestMembership::new(members, revocations),
        );
        if let Some(departure) = departure {
            update = update.answering(departure);
        }
        let advanced = self.repository.advance_owned_space(&update)?;
        let projection = self
            .repository
            .membership_projection(request.local_endpoint_id)
            .map_err(|error| RuntimeError::from(error).after_commit(advanced.revision()))?;
        Ok(RemovedMember {
            revision: advanced.revision(),
            projection_revision: projection.revision(),
            memberships: projection.into_value(),
            chain: advanced.chain().clone(),
            replayed: false,
        })
    }

    /// Answers a departure again with `chain`, committing nothing; `SpaceNotFound` without one.
    fn replay(
        &mut self,
        local_endpoint_id: EndpointId,
        chain: Option<SpaceChain>,
    ) -> Result<RemovedMember, RuntimeError> {
        let chain = chain.ok_or(ma2a_store::StoreError::SpaceNotFound)?;
        let projection = self.repository.membership_projection(local_endpoint_id)?;
        Ok(RemovedMember {
            revision: projection.revision(),
            projection_revision: projection.revision(),
            memberships: projection.into_value(),
            chain,
            replayed: true,
        })
    }

    pub(super) fn advance_owned_space(
        &mut self,
        update: &ma2a_store::OwnedSpaceUpdate,
        local_endpoint_id: EndpointId,
    ) -> Result<(u64, BTreeSet<SpaceId>), RuntimeError> {
        let advanced = self.repository.advance_owned_space(update)?;
        let projection = self
            .repository
            .membership_projection(local_endpoint_id)
            .map_err(|error| RuntimeError::from(error).after_commit(advanced.revision()))?;
        Ok((projection.revision(), projection.into_value()))
    }

    /// Reads the Space memberships the Store currently holds for this Endpoint.
    pub(super) fn memberships(
        &mut self,
        local_endpoint_id: EndpointId,
    ) -> Result<ma2a_store::Committed<BTreeSet<SpaceId>>, RuntimeError> {
        self.repository
            .membership_projection(local_endpoint_id)
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
        let projection = self
            .repository
            .membership_projection(local_endpoint_id)
            .map_err(|error| RuntimeError::from(error).after_commit(revision))?;
        Ok((projection.revision(), projection.into_value()))
    }
}

#[cfg(test)]
#[path = "membership_tests.rs"]
mod tests;
