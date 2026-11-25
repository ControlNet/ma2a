//! Member-side invitation redemption.
//!
//! The exchange with the Space owner runs as a task beside the actor, so a slow or
//! unreachable owner holds up only this request and never the rest of the Runtime.
//! Its answer comes back through the actor loop, which validates and persists it.

use ma2a_core::{EndpointId, SpaceId};
use ma2a_net::NetError;
use ma2a_store::{AddressRecordTarget, AddressRecordValidation, ValidatedAddressRecord};
use tokio::sync::oneshot;

use crate::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EstablishedEnrollment,
    actor::{Actor, Command, RuntimeHandle},
    enrollment::encode_attempt,
    state::RuntimeEvent,
};

type RedemptionReply = oneshot::Sender<Result<EstablishedEnrollment, EnrollmentError>>;

/// Most redemptions that wait on their Space owners at once: as many as the local API serves
/// requests at once, so normally only callers that bypass it, such as an embedding application,
/// meet it.
pub(crate) const MAX_PENDING_REDEMPTIONS: usize = 32;

/// One finished exchange with a Space owner, handed back to the actor loop.
pub(crate) struct RedemptionCompletion {
    /// The Space the invitation names, and the owner that answered for it.
    target: (SpaceId, EndpointId),
    exchanged: Result<(u8, Vec<Vec<u8>>), NetError>,
    reply: RedemptionReply,
}

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
        Ok(self
            .create_enrollment_invite_committed(creation)
            .await?
            .ticket()
            .clone())
    }

    pub(crate) async fn create_enrollment_invite_committed(
        &self,
        creation: EnrollmentCreation,
    ) -> Result<ma2a_store::CreatedEnrollmentInvite, EnrollmentError> {
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
    /// Encodes the attempt, then starts the exchange with the Space owner as a task, unless
    /// as many exchanges are already waiting as are allowed.
    pub(crate) fn redeem_enrollment(
        &mut self,
        attempt: &EnrollmentAttempt,
        reply: RedemptionReply,
    ) {
        if self.redemption_tasks.len() >= MAX_PENDING_REDEMPTIONS {
            let _unsent = reply.send(Err(EnrollmentError::busy()));
            return;
        }
        let owner_addr = attempt.owner_addr();
        let target = (attempt.space_id(), owner_addr.id.into());
        match encode_attempt(attempt) {
            Ok(request) => {
                let exchange = self.endpoint.exchange_redemption(owner_addr, request);
                self.redemption_tasks.spawn(async move {
                    RedemptionCompletion {
                        target,
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

    /// Settles a finished exchange and answers the caller that redeemed the invitation.
    pub(crate) async fn finish_redemption(&mut self, completion: RedemptionCompletion) {
        let RedemptionCompletion {
            target,
            exchanged,
            reply,
        } = completion;
        let result = self.complete_redemption(target, exchanged).await;
        let _unsent = reply.send(result);
    }

    async fn complete_redemption(
        &mut self,
        (expected_space, owner_endpoint_id): (SpaceId, EndpointId),
        exchanged: Result<(u8, Vec<Vec<u8>>), NetError>,
    ) -> Result<EstablishedEnrollment, EnrollmentError> {
        let (status, response_pages) = exchanged.map_err(|error| {
            eprintln!("enrollment transport: {error}");
            EnrollmentError::internal().at_stage(crate::EnrollmentStage::Exchange)
        })?;
        if status != 0 {
            return Err(EnrollmentError::from_status(status)
                .at_stage(crate::EnrollmentStage::OwnerRedemption));
        }
        let bootstrap =
            ma2a_core::EnrollmentBootstrap::decode_frames(&response_pages).map_err(|_| {
                EnrollmentError::from_status(1)
                    .at_stage(crate::EnrollmentStage::BootstrapValidation)
            })?;
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
            return Err(EnrollmentError::from_status(1)
                .at_stage(crate::EnrollmentStage::BootstrapValidation));
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
        .map_err(|_| {
            EnrollmentError::from_status(1).at_stage(crate::EnrollmentStage::BootstrapValidation)
        })?;
        let persisted = self
            .store
            .persist_enrollment(crate::store::EnrollmentPersistence {
                chain,
                owner_address: Box::new(owner_address),
                local_endpoint_id: self.state.endpoint_id,
            })
            .await
            .map_err(|_| {
                EnrollmentError::internal().at_stage(crate::EnrollmentStage::CandidatePersistence)
            })?;
        let chain = persisted.chain;
        // Membership comes from what the Store committed, never from the response.
        // A replayed or otherwise refused chain must not look like a fresh join.
        if !persisted.memberships.contains(&expected_space) {
            return Err(EnrollmentError::from_status(1)
                .at_stage(crate::EnrollmentStage::BootstrapValidation));
        }
        let revision = persisted.revision;
        self.state.memberships = persisted.memberships;
        self.state.revision = revision;
        self.endpoint.set_control_enabled(true);
        self.maintenance
            .enrollment
            .get_or_insert_with(|| crate::enrollment::completion::PendingEnrollment {
                owners: std::collections::BTreeSet::new(),
                stage: crate::EnrollmentStage::ControlLookup,
            })
            .owners
            .insert(owner_endpoint_id);
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        if let Err(error) = self.reconcile_enrollment().await {
            let stage = self
                .maintenance
                .enrollment
                .as_ref()
                .map_or(crate::EnrollmentStage::Publications, |pending| {
                    pending.stage
                });
            eprintln!("enrollment completion failed at {stage:?}: {error}");
            return Err(EnrollmentError::internal()
                .after_commit(revision)
                .at_stage(stage));
        }
        Ok(EstablishedEnrollment::new(revision, chain))
    }
}
