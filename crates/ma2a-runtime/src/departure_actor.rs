//! Member-side Space departure: the leaving member asks the Space authority to
//! sign the next generation without it.
//!
//! The exchange with the authority runs as a task beside the actor, so a slow or
//! unreachable authority holds up only this request and never the rest of the
//! Runtime. Its answer comes back through the actor loop, which validates and
//! persists it.

use ma2a_core::{EnrollmentPage, RequestId, SpaceChain, SpaceId, validate_enrollment_pages};
use ma2a_net::{EndpointAddr, NetError};
use tokio::sync::oneshot;

use crate::{
    SpaceDepartureError,
    actor::{Actor, Command, RuntimeHandle},
    departure::{DEPARTURE_STATUS_OK, accepts_departure, encode_departure},
    state::RuntimeEvent,
};

type DepartureReply = oneshot::Sender<Result<u64, SpaceDepartureError>>;

/// One finished exchange with a Space authority, handed back to the actor loop.
pub(crate) struct DepartureCompletion {
    space_id: SpaceId,
    exchanged: Result<(u8, Vec<Vec<u8>>), NetError>,
    reply: DepartureReply,
}

impl RuntimeHandle {
    /// Asks the Space authority to sign the next generation without this Endpoint.
    ///
    /// # Errors
    /// Returns a typed departure failure when the Space, authority, or persistence rejects it.
    pub async fn leave_space(
        &self,
        space_id: SpaceId,
        request_id: RequestId,
    ) -> Result<u64, SpaceDepartureError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::LeaveSpace {
                space_id,
                request_id,
                reply,
            })
            .await
            .map_err(|_| SpaceDepartureError::internal())?;
        response
            .await
            .map_err(|_| SpaceDepartureError::internal())?
    }
}

impl Actor {
    /// Checks the departure locally, then starts the authority exchange as a task.
    pub(crate) async fn leave_space(
        &mut self,
        (space_id, request_id): (SpaceId, RequestId),
        reply: DepartureReply,
    ) {
        match self.departure_target(space_id).await {
            Ok(owner_addr) => {
                let exchange = self
                    .endpoint
                    .exchange_enrollment(owner_addr, encode_departure(space_id, request_id));
                self.departure_tasks.spawn(async move {
                    DepartureCompletion {
                        space_id,
                        exchanged: exchange.await,
                        reply,
                    }
                });
            }
            Err(error) => {
                let _unsent = reply.send(Err(error));
            }
        }
    }

    /// Returns the authority to ask, and claims the Space for this one departure.
    ///
    /// A second departure of a Space whose exchange is still in flight is refused
    /// rather than sent alongside it.
    async fn departure_target(
        &mut self,
        space_id: SpaceId,
    ) -> Result<EndpointAddr, SpaceDepartureError> {
        let chain = self.member_chain(space_id).await?;
        let authority_owner = chain.genesis().genesis().initial_member().endpoint_id();
        if authority_owner == self.state.endpoint_id {
            return Err(SpaceDepartureError::owner_cannot_leave());
        }
        // The address carries the authority's identity and nothing else on purpose.
        // Iroh then resolves it through this Runtime's installed private
        // `SpaceAddressLookup`, which serves only signed Space address records the
        // Endpoint already holds. Public discovery is disabled, so departure uses
        // the same authenticated target resolution as every other MA2A operation.
        let owner_addr = EndpointAddr::from(
            authority_owner
                .to_public_key()
                .map_err(|_| SpaceDepartureError::internal())?,
        );
        if !self.departing.insert(space_id) {
            return Err(SpaceDepartureError::in_flight());
        }
        Ok(owner_addr)
    }

    /// Loads the Space's chain while the local Endpoint is still one of its members.
    async fn member_chain(&self, space_id: SpaceId) -> Result<SpaceChain, SpaceDepartureError> {
        let chain = self
            .store
            .load_space_chain(space_id)
            .await
            .map_err(|_| SpaceDepartureError::internal())?
            .ok_or_else(SpaceDepartureError::not_a_member)?;
        if !chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == self.state.endpoint_id)
        {
            return Err(SpaceDepartureError::not_a_member());
        }
        Ok(chain)
    }

    /// Settles a finished exchange and answers the caller that asked to leave.
    pub(crate) async fn finish_departure(&mut self, completion: DepartureCompletion) {
        let DepartureCompletion {
            space_id,
            exchanged,
            reply,
        } = completion;
        self.departing.remove(&space_id);
        let result = self.complete_departure(space_id, exchanged).await;
        let _unsent = reply.send(result);
    }

    async fn complete_departure(
        &mut self,
        space_id: SpaceId,
        exchanged: Result<(u8, Vec<Vec<u8>>), NetError>,
    ) -> Result<u64, SpaceDepartureError> {
        let local = self.state.endpoint_id;
        let (status, pages) = exchanged.map_err(|_| SpaceDepartureError::unreachable())?;
        if status != DEPARTURE_STATUS_OK {
            return Err(SpaceDepartureError::rejected());
        }
        // The authority answers with the same bounded page stream enrollment uses,
        // so a long Space history still fits the transport, and the same validator
        // re-verifies genesis, every authority signature, and contiguous ordering.
        let advanced = pages
            .iter()
            .map(|page| EnrollmentPage::decode(page))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| SpaceDepartureError::rejected())?;
        let advanced =
            validate_enrollment_pages(&advanced).map_err(|_| SpaceDepartureError::rejected())?;
        // Membership may have changed while the exchange was in flight, so the
        // answer is checked against the chain held now, not the one asked from.
        let chain = self.member_chain(space_id).await?;
        if !accepts_departure(&chain, &advanced, local) {
            return Err(SpaceDepartureError::rejected());
        }
        let (revision, memberships) = self
            .store
            .persist_departure(advanced, local)
            .await
            .map_err(|error| {
                let error = self.retain_membership_error(error);
                error
                    .committed_revision()
                    .map_or_else(SpaceDepartureError::internal, |revision| {
                        SpaceDepartureError::internal().after_commit(revision)
                    })
            })?;
        self.state.revision = revision;
        self.state.memberships = memberships;
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        self.refresh_after_membership_change()
            .await
            .map_err(|_| SpaceDepartureError::internal().after_commit(revision))?;
        Ok(revision)
    }
}
