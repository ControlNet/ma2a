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
            self.handle_departure_call(call).await;
            return Ok(());
        }
        let Ok((ticket, redemption)) = decode_attempt(call.request(), call.remote_endpoint_id())
        else {
            call.respond(1, Vec::new());
            return Ok(());
        };
        let Ok(now_ms) = self.clock.now_ms() else {
            call.respond(255, Vec::new());
            return Ok(());
        };
        let authorized =
            ma2a_store::AuthorizedEnrollmentRedemption::new(ticket, redemption, now_ms);
        match self.store.redeem_enrollment(authorized).await {
            Ok(EnrollmentOutcome::Redeemed { revision, chain }) => {
                self.state.revision = revision;
                self.maintenance.membership_pending =
                    Some(crate::control_sync::ControlRoundTrigger::EnrollmentCompleted);
                if let Err(error) = self.reconcile_membership_completion().await {
                    eprintln!("enrollment failed at owner_post_commit_projection: {error}");
                    call.respond(255, Vec::new());
                    return Self::absorb_background("owner enrollment completion", Err(error));
                }
                self.respond_with_bootstrap(call, &chain).await;
            }
            Ok(EnrollmentOutcome::Retry { chain }) => {
                if let Err(error) = self.refresh_local_control_publications().await {
                    eprintln!("enrollment failed at owner_retry_publication: {error}");
                    call.respond(255, Vec::new());
                    return Self::absorb_background("owner enrollment retry", Err(error));
                }
                self.respond_with_bootstrap(call, &chain).await;
            }
            Ok(EnrollmentOutcome::Expired) => call.respond(2, Vec::new()),
            Ok(EnrollmentOutcome::Cancelled) => call.respond(3, Vec::new()),
            Ok(EnrollmentOutcome::Conflict) => call.respond(4, Vec::new()),
            Ok(EnrollmentOutcome::NotFound) => call.respond(1, Vec::new()),
            Err(error) => {
                eprintln!("enrollment failed at owner_redemption_transaction: {error}");
                call.respond(255, Vec::new());
                return Self::absorb_background("owner redemption", Err(error));
            }
        }
        Ok(())
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
        if let Ok(frames) = bootstrap {
            call.respond(0, frames);
        } else {
            eprintln!("enrollment failed at owner_bootstrap_construction");
            call.respond(255, Vec::new());
        }
    }
}
