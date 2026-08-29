use ma2a_net::EnrollmentCall;
use ma2a_store::EnrollmentOutcome;
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
            .send(Command::RedeemEnrollment { attempt, reply })
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
        _now_ms: u64,
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
        let request = encode_attempt(&attempt)?;
        let (status, response) = self
            .endpoint
            .exchange_enrollment(attempt.owner_addr(), &request)
            .await
            .map_err(|_| EnrollmentError::internal())?;
        if status != 0 {
            return Err(EnrollmentError::from_status(status));
        }
        let (revision, chain) = self
            .store
            .persist_enrollment(response)
            .await
            .map_err(|_| EnrollmentError::internal())?;
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
        self.state.memberships.insert(chain.space_id());
        self.state.revision = revision;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(EstablishedEnrollment::new(chain.latest_generation()))
    }

    pub(crate) async fn handle_enrollment_call(&mut self, call: EnrollmentCall) {
        let Ok((ticket, redemption)) = decode_attempt(call.request(), call.remote_endpoint_id())
        else {
            call.respond(1, Vec::new());
            return;
        };
        match self.store.redeem_enrollment(ticket, redemption).await {
            Ok(EnrollmentOutcome::Redeemed { revision, chain }) => {
                self.state.revision = revision;
                call.respond(0, chain);
            }
            Ok(EnrollmentOutcome::Retry { chain }) => call.respond(0, chain),
            Ok(EnrollmentOutcome::Expired) => call.respond(2, Vec::new()),
            Ok(EnrollmentOutcome::Cancelled) => call.respond(3, Vec::new()),
            Ok(EnrollmentOutcome::Conflict) => call.respond(4, Vec::new()),
            Ok(EnrollmentOutcome::NotFound) => call.respond(1, Vec::new()),
            Err(_) => call.respond(255, Vec::new()),
        }
    }
}
