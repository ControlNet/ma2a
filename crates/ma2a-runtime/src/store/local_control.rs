use super::{RelayPublicationResult, StoreBackend};
use crate::error::{RuntimeError, RuntimeErrorKind};

pub(super) struct PendingRelayPublication {
    config: ma2a_net::PrivateRelayProviderConfig,
    authorizations: Vec<ma2a_core::SpaceAuthorizationView>,
    advertisements: Vec<ma2a_core::SignedPrivateRelayAdvertisementV1>,
    issued_at_ms: u64,
    expires_at_ms: u64,
}

impl PendingRelayPublication {
    fn can_reuse(
        &self,
        desired: (
            &ma2a_net::PrivateRelayProviderConfig,
            &[ma2a_core::SpaceAuthorizationView],
        ),
        now_ms: u64,
    ) -> Result<bool, RuntimeError> {
        let (config, authorizations) = desired;
        let (issued_at_ms, expires_at_ms) = self.effective_window()?;
        if now_ms < issued_at_ms {
            return Err(RuntimeError::new(RuntimeErrorKind::Clock));
        }
        Ok(self.config == *config
            && self.authorizations == authorizations
            && crate::relay_publication::is_current(now_ms, issued_at_ms, expires_at_ms))
    }

    fn effective_window(&self) -> Result<(u64, u64), RuntimeError> {
        let window =
            self.advertisements
                .first()
                .map_or((self.issued_at_ms, self.expires_at_ms), |signed| {
                    let advertisement = signed.advertisement();
                    (advertisement.issued_at_ms(), advertisement.expires_at_ms())
                });
        if self.advertisements.iter().any(|signed| {
            (
                signed.advertisement().issued_at_ms(),
                signed.advertisement().expires_at_ms(),
            ) != window
        }) {
            return Err(RuntimeError::new(RuntimeErrorKind::Control));
        }
        Ok(window)
    }
}

#[cfg(test)]
pub(crate) enum RelayPublicationTestCommand {
    FailAfter(usize, tokio::sync::oneshot::Sender<()>),
    Pending(tokio::sync::oneshot::Sender<bool>),
}

#[cfg(test)]
impl StoreBackend {
    pub(super) fn dispatch_relay_publication_test(&mut self, command: RelayPublicationTestCommand) {
        match command {
            RelayPublicationTestCommand::FailAfter(committed_spaces, reply) => {
                self.relay_publication_fail_after = Some(committed_spaces);
                let _unsent = reply.send(());
            }
            RelayPublicationTestCommand::Pending(reply) => {
                let _unsent = reply.send(self.pending_relay_publication.is_some());
            }
        }
    }
}

pub(super) fn authorize(
    repository: &ma2a_store::Repository,
    input: &crate::control_sync::ControlAuthorizationInput,
    reply: tokio::sync::oneshot::Sender<Result<(), ma2a_net::ControlRejection>>,
) {
    let result = crate::control_sync::authorize(repository, input);
    let _unsent = reply.send(result);
}

pub(super) fn respond(
    repository: &mut ma2a_store::Repository,
    input: &crate::control_sync::ControlExchangeInput,
    reply: tokio::sync::oneshot::Sender<
        Result<crate::control_sync::ControlRespondOutcome, ma2a_net::ControlRejection>,
    >,
) {
    let result = crate::control_sync::respond(repository, input);
    let _unsent = reply.send(result);
}

impl StoreBackend {
    pub(super) fn reconcile_relay_activity(
        &mut self,
        local_endpoint_id: ma2a_core::EndpointId,
        active_spaces: &[ma2a_core::SpaceId],
    ) -> Result<(u64, bool), RuntimeError> {
        self.repository
            .reconcile_private_relay_activity(local_endpoint_id, active_spaces)
            .map_err(Into::into)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "publication inputs map directly to the signed record"
    )]
    pub(super) fn publish_address(
        &mut self,
        publisher: &ma2a_net::AddressPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
        force_advance: bool,
    ) -> Result<(u64, bool), RuntimeError> {
        let states = self.repository.control_spaces_for(local_endpoint_id)?;
        let mut advanced = false;
        for state in states {
            let authorization = state.authorization();
            let request = ma2a_net::AddressPublishRequest::new(&authorization, now_ms);
            let request = if force_advance {
                request.force_advance()
            } else {
                request
            };
            advanced |= publisher
                .publish(&mut self.repository, request)
                .map_err(RuntimeError::from)?
                .is_some();
        }
        Ok((self.repository.revision()?, advanced))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "publication inputs map directly to the signed advertisement"
    )]
    pub(super) fn publish_relay_advertisements(
        &mut self,
        publisher: &ma2a_net::PrivateRelayAdvertisementPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<RelayPublicationResult, RuntimeError> {
        let authorizations = self
            .repository
            .control_spaces_for(local_endpoint_id)?
            .iter()
            .map(ma2a_store::ControlSpaceState::authorization)
            .collect::<Vec<_>>();
        let reuse = self
            .pending_relay_publication
            .as_ref()
            .map_or(Ok(false), |pending| {
                pending.can_reuse((publisher.config(), &authorizations), issued_at_ms)
            })?;
        if !reuse {
            // A changed or renewal-due retained batch cannot complete as a
            // safely fresh publication. The signed replacement becomes pending
            // before any per-Space write.
            self.pending_relay_publication = None;
            let advertisements = publisher
                .advertisements(
                    &mut self.repository,
                    ma2a_net::AdvertisementPublicationRequest::new(
                        &authorizations,
                        issued_at_ms,
                        expires_at_ms,
                    ),
                )
                .map_err(RuntimeError::from)?;
            self.pending_relay_publication = Some(PendingRelayPublication {
                config: publisher.config().clone(),
                authorizations: authorizations.clone(),
                advertisements,
                issued_at_ms,
                expires_at_ms,
            });
        }
        let pending = self
            .pending_relay_publication
            .as_ref()
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Control))?;
        let advertisements = &pending.advertisements;
        let (effective_issued_at_ms, effective_expires_at_ms) = pending.effective_window()?;
        let active_spaces = advertisements
            .iter()
            .map(|advertisement| advertisement.advertisement().space_id())
            .collect::<Vec<_>>();
        let mut advanced = false;
        for (committed_spaces, advertisement) in advertisements.iter().enumerate() {
            #[cfg(test)]
            if self.relay_publication_fail_after == Some(committed_spaces) {
                self.relay_publication_fail_after = None;
                return Err(
                    ma2a_store::StoreError::Io(std::io::ErrorKind::Interrupted.into()).into(),
                );
            }
            #[cfg(not(test))]
            let _ = committed_spaces;
            let authorization = authorizations
                .iter()
                .find(|authorization| {
                    authorization.space_id() == advertisement.advertisement().space_id()
                })
                .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Control))?;
            let validated = ma2a_store::ValidatedRelayAdvertisement::parse(
                advertisement.canonical_bytes(),
                authorization,
                issued_at_ms,
            )
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
            match self
                .repository
                .advance_private_relay_advertisement(&validated)?
            {
                ma2a_store::RelayAdvertisementOutcome::Advanced { .. } => advanced = true,
                ma2a_store::RelayAdvertisementOutcome::Idempotent { .. } => {}
                ma2a_store::RelayAdvertisementOutcome::Rollback { .. }
                | ma2a_store::RelayAdvertisementOutcome::Fork { .. } => {
                    return Err(RuntimeError::new(RuntimeErrorKind::Control));
                }
            }
        }
        let (revision, activity_changed) = self
            .repository
            .reconcile_private_relay_activity(local_endpoint_id, &active_spaces)?;
        self.pending_relay_publication = None;
        Ok(RelayPublicationResult {
            revision,
            changed: advanced || activity_changed,
            issued_at_ms: effective_issued_at_ms,
            expires_at_ms: effective_expires_at_ms,
        })
    }
}
