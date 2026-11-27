use crate::{actor::Actor, enrollment::decode_attempt};
use ma2a_net::EnrollmentCall;
use ma2a_store::EnrollmentOutcome;

impl Actor {
    pub(crate) async fn handle_enrollment_call(
        &mut self,
        call: EnrollmentCall,
    ) -> Result<(), crate::RuntimeError> {
        // Departure shares the bootstrap ALPN and is distinguished by its magic prefix.
        if call
            .request()
            .starts_with(&crate::departure::DEPARTURE_MAGIC)
        {
            return self.handle_departure_call(call).await;
        }
        let Ok((ticket, redemption)) = decode_attempt(call.request(), call.remote_endpoint_id())
        else {
            call.respond(1, Vec::new());
            return Ok(());
        };
        let outcome = match self
            .redeem_at_current_clock(ticket.clone(), redemption.clone())
            .await
        {
            Ok(EnrollmentOutcome::OwnerAddressUnavailable) => {
                match self.publish_local_address().await {
                    Ok(_) => self.redeem_at_current_clock(ticket, redemption).await,
                    Err(error) => {
                        let status = if error.is_deferred_publication() {
                            6
                        } else {
                            255
                        };
                        call.respond(status, Vec::new());
                        return Self::absorb_background(
                            "owner pre-enrollment address publication",
                            Err(error),
                        );
                    }
                }
            }
            other => other,
        };
        match outcome {
            Ok(EnrollmentOutcome::Redeemed { revision, chain }) => {
                self.state.revision = revision;
                // The Space has a new member, so no peer is synchronized with it yet.
                self.synchronized_control_peers.clear();
                let _receivers = self
                    .events
                    .send(crate::state::RuntimeEvent::memberships_changed(revision));
                self.maintenance.membership_pending =
                    Some(crate::control_sync::ControlRoundTrigger::EnrollmentCompleted);
                let projected = self.reconcile_membership_completion().await;
                if let Some(call) = Self::accept_enrollment_projection(
                    call,
                    (
                        "owner enrollment completion",
                        "owner_post_commit_projection",
                    ),
                    projected,
                )? {
                    self.respond_with_bootstrap(call, &chain).await?;
                }
            }
            Ok(EnrollmentOutcome::Retry { chain }) => {
                let published = self.refresh_local_control_publications().await;
                if let Some(call) = Self::accept_enrollment_projection(
                    call,
                    ("owner enrollment retry", "owner_retry_publication"),
                    published,
                )? {
                    self.respond_with_bootstrap(call, &chain).await?;
                }
            }
            Ok(EnrollmentOutcome::Expired) => call.respond(2, Vec::new()),
            Ok(EnrollmentOutcome::Cancelled) => call.respond(3, Vec::new()),
            Ok(EnrollmentOutcome::Conflict) => call.respond(4, Vec::new()),
            Ok(EnrollmentOutcome::OwnerAddressUnavailable) => call.respond(6, Vec::new()),
            Ok(EnrollmentOutcome::NotFound) => call.respond(1, Vec::new()),
            Err(error) if error.is_space_members_full() => call.respond(5, Vec::new()),
            Err(error) => {
                eprintln!("enrollment failed at owner_redemption_transaction: {error}");
                call.respond(255, Vec::new());
                return Self::absorb_background("owner redemption", Err(error));
            }
        }
        Ok(())
    }

    fn accept_enrollment_projection(
        call: EnrollmentCall,
        (context, label): (&str, &str),
        result: Result<(), crate::RuntimeError>,
    ) -> Result<Option<EnrollmentCall>, crate::RuntimeError> {
        match result {
            Ok(()) => Ok(Some(call)),
            Err(error) if error.is_deferred_publication() => {
                Self::absorb_background(context, Err(error))?;
                Ok(Some(call))
            }
            Err(error) => {
                eprintln!("enrollment failed at {label}: {error}");
                call.respond(255, Vec::new());
                Self::absorb_background(context, Err(error))?;
                Ok(None)
            }
        }
    }

    async fn redeem_at_current_clock(
        &self,
        ticket: ma2a_core::SignedInviteTicket,
        redemption: ma2a_store::EnrollmentRedemption,
    ) -> Result<EnrollmentOutcome, crate::RuntimeError> {
        let (now_ms, _) = self.enrollment_now_ms()?;
        let authorized =
            ma2a_store::AuthorizedEnrollmentRedemption::new(ticket, redemption, now_ms)
                .requiring_owner_address(self.state.endpoint_id);
        self.store.redeem_enrollment(authorized).await
    }

    fn enrollment_now_ms(&self) -> Result<(i64, u64), crate::RuntimeError> {
        let now_ms = self.clock.now_ms()?;
        let now_u64 = u64::try_from(now_ms)
            .map_err(|_| crate::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        Ok((now_ms, now_u64))
    }

    async fn respond_with_bootstrap(
        &self,
        call: EnrollmentCall,
        bytes: &[u8],
    ) -> Result<(), crate::RuntimeError> {
        let bootstrap = async {
            let chain = ma2a_core::SpaceChain::import_public(bytes)
                .map_err(ma2a_store::StoreError::from)?;
            let persisted = self
                .store
                .address_record(chain.space_id(), self.state.endpoint_id)
                .await?
                .ok_or_else(|| {
                    crate::RuntimeError::new(
                        crate::error::RuntimeErrorKind::AddressObservationUnavailable,
                    )
                })?;
            let (_, now_ms) = self.enrollment_now_ms()?;
            let authorization = ma2a_core::SpaceAuthorizationView::from_chain(&chain);
            let owner_address = ma2a_store::ValidatedAddressRecord::parse(
                persisted.signed_record(),
                ma2a_store::AddressRecordValidation::new(
                    ma2a_store::AddressRecordTarget::new(chain.space_id(), self.state.endpoint_id),
                    &authorization,
                    now_ms,
                ),
            )
            .map_err(|_| {
                crate::RuntimeError::new(
                    crate::error::RuntimeErrorKind::AddressObservationUnavailable,
                )
            })?
            .record()
            .clone();
            ma2a_core::EnrollmentBootstrap::new(chain, owner_address)
                .and_then(|bootstrap| bootstrap.encode_frames())
                .map_err(|error| crate::RuntimeError::from(ma2a_store::StoreError::from(error)))
        }
        .await;
        match bootstrap {
            Ok(frames) => call.respond(0, frames),
            Err(error) => {
                let status = if error.is_deferred_publication() {
                    6
                } else {
                    255
                };
                eprintln!("enrollment failed at owner_bootstrap_construction: {error}");
                call.respond(status, Vec::new());
                Self::absorb_background("owner bootstrap construction", Err(error))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "owner_stepping_clock_test.rs"]
mod stepping_clock_test;
