//! Actor-side Space departure: the leaving member's request and the Space
//! authority's signed removal.

use ma2a_core::{EndpointId, RequestId, SpaceChain, SpaceId};
use ma2a_net::EnrollmentCall;
use tokio::sync::oneshot;

use crate::{
    SpaceDepartureError,
    actor::{Actor, Command, RuntimeHandle},
    departure::{
        DEPARTURE_STATUS_INTERNAL, DEPARTURE_STATUS_OK, DEPARTURE_STATUS_REJECTED,
        accepts_departure, decode_departure, encode_departure,
    },
    state::RuntimeEvent,
};

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
    pub(crate) async fn leave_space(
        &mut self,
        space_id: SpaceId,
        request_id: RequestId,
    ) -> Result<u64, SpaceDepartureError> {
        let local = self.state.endpoint_id;
        let chain = self
            .store
            .load_space_chain(space_id)
            .await
            .map_err(|_| SpaceDepartureError::internal())?
            .ok_or_else(SpaceDepartureError::not_a_member)?;
        if !chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == local)
        {
            return Err(SpaceDepartureError::not_a_member());
        }
        let authority_owner = chain.genesis().genesis().initial_member().endpoint_id();
        if authority_owner == local {
            return Err(SpaceDepartureError::owner_cannot_leave());
        }
        let owner_addr = ma2a_net::EndpointAddr::from(
            authority_owner
                .to_public_key()
                .map_err(|_| SpaceDepartureError::internal())?,
        );
        let (status, pages) = self
            .endpoint
            .exchange_enrollment(owner_addr, &encode_departure(space_id, request_id))
            .await
            .map_err(|_| SpaceDepartureError::unreachable())?;
        if status != DEPARTURE_STATUS_OK {
            return Err(SpaceDepartureError::rejected());
        }
        let [advanced] = pages.as_slice() else {
            return Err(SpaceDepartureError::rejected());
        };
        let advanced =
            SpaceChain::import_public(advanced).map_err(|_| SpaceDepartureError::rejected())?;
        if !accepts_departure(&chain, &advanced, local) {
            return Err(SpaceDepartureError::rejected());
        }
        let (revision, memberships) = self
            .store
            .persist_departure(advanced, local)
            .await
            .map_err(|_| SpaceDepartureError::internal())?;
        self.state.revision = revision;
        self.state.memberships = memberships;
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        self.refresh_after_membership_change()
            .await
            .map_err(|_| SpaceDepartureError::internal())?;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(revision)
    }

    /// Handles a departure request that arrived on the shared bootstrap ALPN.
    ///
    /// The transport authenticates the remote Endpoint, so a request can only
    /// ever remove its own sender.
    pub(crate) async fn handle_departure_call(&mut self, call: EnrollmentCall) {
        let departing = call.remote_endpoint_id();
        let Some((space_id, _request_id)) = decode_departure(call.request()) else {
            call.respond(DEPARTURE_STATUS_REJECTED, Vec::new());
            return;
        };
        match self.remove_departing_member(space_id, departing).await {
            Ok(Some(chain)) => match chain.export_public() {
                Ok(bytes) => call.respond(DEPARTURE_STATUS_OK, vec![bytes]),
                Err(_) => call.respond(DEPARTURE_STATUS_INTERNAL, Vec::new()),
            },
            Ok(None) => call.respond(DEPARTURE_STATUS_REJECTED, Vec::new()),
            Err(()) => call.respond(DEPARTURE_STATUS_INTERNAL, Vec::new()),
        }
    }

    /// Returns the advanced chain, `Ok(None)` when the request is not authorized,
    /// and `Err(())` for local failures the requester must not distinguish.
    async fn remove_departing_member(
        &mut self,
        space_id: SpaceId,
        departing: EndpointId,
    ) -> Result<Option<SpaceChain>, ()> {
        let Ok(Some(chain)) = self.store.load_space_chain(space_id).await else {
            return Ok(None);
        };
        if chain.genesis().genesis().initial_member().endpoint_id() == departing {
            return Ok(None);
        }
        if !chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == departing)
        {
            // A repeated departure converges on the chain that already removed it.
            return Ok(chain
                .revocations()
                .iter()
                .any(|revocation| revocation.endpoint_id() == departing)
                .then_some(chain));
        }
        let Ok(issued_at_ms) = self.clock.now_ms().map(u64::try_from) else {
            return Err(());
        };
        let Ok(issued_at_ms) = issued_at_ms else {
            return Err(());
        };
        let removed = self
            .store
            .revoke_owned_space_member(crate::store::OwnedMemberRevocation {
                space_id,
                endpoint_id: departing,
                issued_at_ms,
                local_endpoint_id: self.state.endpoint_id,
            })
            .await
            .map_err(|_| ())?;
        self.state.revision = removed.revision;
        self.state.memberships = removed.memberships;
        self.synchronized_control_peers.clear();
        self.refresh_after_membership_change()
            .await
            .map_err(|_| ())?;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(removed.revision));
        Ok(Some(removed.chain))
    }
}
