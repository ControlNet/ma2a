//! Authority-side Space departure: the signed removal a Space authority answers
//! a leaving member's request with.

use ma2a_core::{EndpointId, EnrollmentPage, RequestId, SpaceChain, SpaceId};
use ma2a_net::EnrollmentCall;

use crate::{
    actor::Actor,
    departure::{
        DEPARTURE_STATUS_INTERNAL, DEPARTURE_STATUS_OK, DEPARTURE_STATUS_REJECTED, decode_departure,
    },
    state::RuntimeEvent,
};

impl Actor {
    /// Handles a departure request that arrived on the shared bootstrap ALPN.
    ///
    /// The transport authenticates the remote Endpoint, so a request can only
    /// ever remove its own sender.
    pub(crate) async fn handle_departure_call(
        &mut self,
        call: EnrollmentCall,
    ) -> Result<(), crate::RuntimeError> {
        let departing = call.remote_endpoint_id();
        let Some((space_id, request_id)) = decode_departure(call.request()) else {
            call.respond(DEPARTURE_STATUS_REJECTED, Vec::new());
            return Ok(());
        };
        match self
            .remove_departing_member(space_id, (departing, request_id))
            .await
        {
            Ok(Some(chain)) => match departure_pages(&chain) {
                Ok(pages) => call.respond(DEPARTURE_STATUS_OK, pages),
                Err(()) => call.respond(DEPARTURE_STATUS_INTERNAL, Vec::new()),
            },
            Ok(None) => call.respond(DEPARTURE_STATUS_REJECTED, Vec::new()),
            Err(error) => {
                call.respond(DEPARTURE_STATUS_INTERNAL, Vec::new());
                Self::absorb_background("owner departure completion", Err(error))?;
            }
        }
        Ok(())
    }
}

/// Splits the advanced chain into the bounded pages the transport accepts.
///
/// Departure reuses enrollment's pagination rather than shipping one whole-chain
/// frame, because a long-lived Space's history does not fit a single frame.
fn departure_pages(chain: &SpaceChain) -> Result<Vec<Vec<u8>>, ()> {
    EnrollmentPage::paginate(chain)
        .map_err(|_| ())?
        .iter()
        .map(|page| page.encode().map_err(|_| ()))
        .collect()
}

impl Actor {
    /// Returns the advanced chain, `Ok(None)` when the request is not authorized,
    /// and a typed local failure kept separate from remote denial.
    async fn remove_departing_member(
        &mut self,
        space_id: SpaceId,
        (departing, request_id): (EndpointId, RequestId),
    ) -> Result<Option<SpaceChain>, crate::RuntimeError> {
        let Some(chain) = self.store.load_space_chain(space_id).await? else {
            return Ok(None);
        };
        // A member holding a copy of the chain is not the one that signs; it refuses without
        // touching anything rather than failing on the authority key it does not hold.
        if !crate::departure::answers_departure(&chain, self.state.endpoint_id, departing) {
            return Ok(None);
        }
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| crate::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        let removed = self
            .store
            .revoke_owned_space_member(crate::store::OwnedMemberRevocation {
                space_id,
                endpoint_id: departing,
                issued_at_ms,
                local_endpoint_id: self.state.endpoint_id,
                request_id: Some(request_id),
            })
            .await;
        let removed = match removed {
            Ok(removed) => removed,
            // Neither a member now nor removed by any earlier answer, or this Endpoint created
            // the Space but does not hold its authority key: nothing to say.
            Err(error) if error.is_space_not_found() || error.is_not_space_owner() => {
                return Ok(None);
            }
            Err(error) => return Err(self.retain_membership_error(error)),
        };
        if removed.replayed {
            return Ok(Some(removed.chain));
        }
        self.state.revision = removed.projection_revision;
        self.state.memberships = removed.memberships;
        self.synchronized_control_peers.clear();
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(removed.revision));
        if let Err(error) = self.refresh_after_membership_change().await {
            if error.is_deferred_publication() {
                // Lookup, relay candidates, and private relay access already ran; only local
                // publication waits for maintenance. Schedule the manifest round now and return
                // the committed removal chain to the departing member.
                if let Some(trigger) = self.maintenance.membership_pending.take() {
                    self.schedule_control_round(trigger, None);
                }
                Self::absorb_background("owner departure publication", Err(error))?;
            } else {
                return Err(error.after_commit(removed.revision));
            }
        }
        Ok(Some(removed.chain))
    }
}
