use super::{AddressPublicationResult, RelayExpiryPolicy, RelayPublicationResult, StoreBackend};
use crate::error::{RuntimeError, RuntimeErrorKind};

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
    reply: tokio::sync::oneshot::Sender<Result<(), crate::control_sync::ControlFailure>>,
) {
    let result = crate::control_sync::authorize(repository, input);
    let _unsent = reply.send(result);
}

pub(super) fn respond(
    repository: &mut ma2a_store::Repository,
    input: &crate::control_sync::ControlExchangeInput,
    reply: tokio::sync::oneshot::Sender<
        Result<crate::control_sync::ControlRespondOutcome, crate::control_sync::ControlFailure>,
    >,
) {
    let result = crate::control_sync::respond(repository, input);
    let _unsent = reply.send(result);
}

pub(super) fn apply(
    repository: &mut ma2a_store::Repository,
    (input, requested): &(
        crate::control_sync::ControlExchangeInput,
        Vec<ma2a_core::SpaceId>,
    ),
    reply: tokio::sync::oneshot::Sender<
        Result<crate::control_sync::ControlApplyOutcome, crate::control_sync::ControlFailure>,
    >,
) {
    let result = crate::control_sync::apply_response(repository, input, requested);
    let _unsent = reply.send(result);
}

impl StoreBackend {
    pub(super) fn reconcile_relay_activity(
        &mut self,
        local_endpoint_id: ma2a_core::EndpointId,
        active_spaces: &[ma2a_core::SpaceId],
    ) -> Result<(u64, bool), RuntimeError> {
        self.pending_relay_publication = None;
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
    ) -> Result<AddressPublicationResult, RuntimeError> {
        let states = self.repository.control_spaces_for(local_endpoint_id)?;
        let mut advanced = false;
        let mut clock_behind = false;
        for state in states {
            let authorization = state.authorization();
            let request = ma2a_net::AddressPublishRequest::new(&authorization, now_ms);
            match publisher.publish(&mut self.repository, request) {
                Ok(record) => advanced |= record.is_some(),
                Err(ma2a_net::AddressPublisherError::ClockRollback) => clock_behind = true,
                Err(error) => return Err(RuntimeError::from(error)),
            }
        }
        Ok(AddressPublicationResult {
            revision: self.repository.revision()?,
            advanced,
            clock_behind,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "publication inputs map directly to the signed advertisement"
    )]
    pub(super) fn publish_relay_advertisements(
        &mut self,
        publisher: &ma2a_net::PrivateRelayAdvertisementPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        (issued_at_ms, expires_at_ms, expiry_policy): (u64, u64, RelayExpiryPolicy),
        skip_spaces: &std::collections::BTreeSet<ma2a_core::SpaceId>,
    ) -> Result<RelayPublicationResult, RuntimeError> {
        let authorizations = self
            .repository
            .control_spaces_for(local_endpoint_id)?
            .iter()
            .map(ma2a_store::ControlSpaceState::authorization)
            .collect::<Vec<_>>();
        let (_pre_revision, pre_activity_changed) = self.reconcile_active_effective_relay_spaces(
            local_endpoint_id,
            (publisher.config(), &authorizations),
        )?;
        self.prepare_pending_relay_publication(
            (publisher, &authorizations),
            (issued_at_ms, expires_at_ms, expiry_policy, skip_spaces),
        )?;
        let pending = self
            .pending_relay_publication
            .as_ref()
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Control))?;
        let advertisements = &pending.advertisements;
        let active_spaces = &pending.active_spaces;
        let clock_behind = pending.clock_behind;
        let (effective_issued_at_ms, effective_expires_at_ms) = pending.effective_window()?;
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
            .reconcile_private_relay_activity(local_endpoint_id, active_spaces)?;
        let published_spaces = advertisements
            .iter()
            .map(|advertisement| advertisement.advertisement().space_id())
            .collect::<Vec<_>>();
        self.pending_relay_publication = None;
        Ok(RelayPublicationResult {
            revision,
            changed: advanced || pre_activity_changed || activity_changed,
            clock_behind,
            issued_at_ms: effective_issued_at_ms,
            expires_at_ms: effective_expires_at_ms,
            published_spaces,
        })
    }
}
