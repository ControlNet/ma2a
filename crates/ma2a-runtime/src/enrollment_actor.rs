use ma2a_net::EnrollmentCall;
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, EnrollmentOutcome, ValidatedAddressRecord,
};
use tokio::sync::oneshot;

use crate::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EstablishedEnrollment,
    actor::{Actor, Command, RuntimeHandle},
    enrollment::{decode_attempt, encode_attempt},
    state::RuntimeEvent,
};

impl RuntimeHandle {
    /// Creates and durably records one owner-authorized single-use invitation.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime or durable invitation issuance fails.
    pub async fn create_enrollment_invite(
        &self,
        creation: EnrollmentCreation,
    ) -> Result<ma2a_core::SignedInviteTicket, EnrollmentError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::CreateEnrollmentInvite { creation, reply })
            .await
            .map_err(|_| EnrollmentError::internal())?;
        response.await.map_err(|_| EnrollmentError::internal())?
    }

    /// Redeems an invitation and establishes local membership only after durable chain persistence.
    ///
    /// # Errors
    ///
    /// Returns a typed denial or internal error when exchange or persistence fails.
    pub async fn redeem_enrollment(
        self,
        attempt: EnrollmentAttempt,
    ) -> Result<EstablishedEnrollment, EnrollmentError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RedeemEnrollment {
                attempt: Box::new(attempt),
                reply,
            })
            .await
            .map_err(|_| EnrollmentError::internal())?;
        response.await.map_err(|_| EnrollmentError::internal())?
    }

    /// Cancels one still-pending invitation.
    ///
    /// # Errors
    ///
    /// Returns an error when the invitation is no longer pending or persistence fails.
    pub async fn cancel_enrollment_invite(
        &self,
        invitation_id: [u8; 16],
    ) -> Result<(), EnrollmentError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::CancelEnrollmentInvite {
                invitation_id,
                reply,
            })
            .await
            .map_err(|_| EnrollmentError::internal())?;
        response.await.map_err(|_| EnrollmentError::internal())?
    }
}

impl Actor {
    pub(crate) async fn redeem_enrollment(
        &mut self,
        attempt: EnrollmentAttempt,
    ) -> Result<EstablishedEnrollment, EnrollmentError> {
        let expected_space = attempt.space_id();
        let owner_addr = attempt.owner_addr();
        let owner_endpoint_id = owner_addr.id.into();
        let request = encode_attempt(&attempt)?;
        let (status, response_pages) = self
            .endpoint
            .exchange_enrollment(owner_addr, &request)
            .await
            .map_err(|_| EnrollmentError::internal())?;
        if status != 0 {
            return Err(EnrollmentError::from_status(status));
        }
        let bootstrap = ma2a_core::EnrollmentBootstrap::decode_frames(&response_pages)
            .map_err(|_| EnrollmentError::from_status(1))?;
        let (chain, owner_address) = bootstrap.into_parts();
        if chain.space_id() != expected_space
            || chain
                .members()
                .binary_search_by_key(
                    &self.state.endpoint_id,
                    ma2a_core::SpaceMemberV1::endpoint_id,
                )
                .is_err()
        {
            return Err(EnrollmentError::from_status(1));
        }
        let now_ms = u64::try_from(
            self.clock
                .now_ms()
                .map_err(|_| EnrollmentError::internal())?,
        )
        .map_err(|_| EnrollmentError::internal())?;
        let authorization = ma2a_core::SpaceAuthorizationView::from_chain(&chain);
        let owner_address = ValidatedAddressRecord::parse(
            owner_address.canonical_bytes(),
            AddressRecordValidation::new(
                AddressRecordTarget::new(expected_space, owner_endpoint_id),
                &authorization,
                now_ms,
            ),
        )
        .map_err(|_| EnrollmentError::from_status(1))?;
        let (revision, chain) = self
            .store
            .persist_enrollment(chain, owner_address)
            .await
            .map_err(|_| EnrollmentError::internal())?;
        self.state.memberships.insert(chain.space_id());
        self.state.revision = revision;
        self.endpoint.set_control_enabled(true);
        self.refresh_control_lookup()
            .await
            .map_err(|_| EnrollmentError::internal())?;
        self.refresh_relay_candidates()
            .await
            .map_err(|_| EnrollmentError::internal())?;
        self.refresh_local_control_publications()
            .await
            .map_err(|_| EnrollmentError::internal())?;
        self.schedule_control_round(
            crate::control_sync::ControlRoundTrigger::Explicit(
                crate::control_sync::ControlRoundScope::peer(owner_endpoint_id),
            ),
            None,
        );
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(EstablishedEnrollment::new(chain.latest_generation()))
    }

    pub(crate) async fn handle_enrollment_call(&mut self, call: EnrollmentCall) {
        // Departure shares the bootstrap ALPN and is distinguished by its magic prefix.
        if call
            .request()
            .starts_with(&crate::departure::DEPARTURE_MAGIC)
        {
            self.handle_departure_call(call).await;
            return;
        }
        let Ok((ticket, redemption)) = decode_attempt(call.request(), call.remote_endpoint_id())
        else {
            call.respond(1, Vec::new());
            return;
        };
        let Ok(now_ms) = self.clock.now_ms() else {
            call.respond(255, Vec::new());
            return;
        };
        let authorized =
            ma2a_store::AuthorizedEnrollmentRedemption::new(ticket, redemption, now_ms);
        match self.store.redeem_enrollment(authorized).await {
            Ok(EnrollmentOutcome::Redeemed { revision, chain }) => {
                self.state.revision = revision;
                if self.refresh_control_lookup().await.is_ok()
                    && self.refresh_relay_candidates().await.is_ok()
                {
                    self.schedule_control_round(
                        crate::control_sync::ControlRoundTrigger::EnrollmentCompleted,
                        None,
                    );
                }
                self.respond_with_bootstrap(call, &chain).await;
            }
            Ok(EnrollmentOutcome::Retry { chain }) => {
                self.respond_with_bootstrap(call, &chain).await;
            }
            Ok(EnrollmentOutcome::Expired) => call.respond(2, Vec::new()),
            Ok(EnrollmentOutcome::Cancelled) => call.respond(3, Vec::new()),
            Ok(EnrollmentOutcome::Conflict) => call.respond(4, Vec::new()),
            Ok(EnrollmentOutcome::NotFound) => call.respond(1, Vec::new()),
            Err(_) => call.respond(255, Vec::new()),
        }
    }

    async fn respond_with_bootstrap(&self, call: EnrollmentCall, bytes: &[u8]) {
        let bootstrap = async {
            let chain = ma2a_core::SpaceChain::import_public(bytes)
                .map_err(|_| ma2a_core::ProtocolError::INVALID_INPUT)?;
            let persisted = self
                .store
                .address_record(chain.space_id(), self.state.endpoint_id)
                .await
                .map_err(|_| ma2a_core::ProtocolError::INTERNAL)?
                .ok_or(ma2a_core::ProtocolError::INTERNAL)?;
            let owner_address = ma2a_core::SignedSpaceAddressRecordV1::parse_canonical_bytes(
                persisted.signed_record(),
            )?;
            ma2a_core::EnrollmentBootstrap::new(chain, owner_address)?.encode_frames()
        }
        .await;
        match bootstrap {
            Ok(frames) => call.respond(0, frames),
            Err(_) => call.respond(255, Vec::new()),
        }
    }
}
