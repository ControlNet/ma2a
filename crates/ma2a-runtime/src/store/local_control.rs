use super::StoreBackend;
use crate::error::{RuntimeError, RuntimeErrorKind};

pub(super) struct PendingRelayPublication {
    config: ma2a_net::PrivateRelayProviderConfig,
    authorizations: Vec<ma2a_core::SpaceAuthorizationView>,
    advertisements: Vec<ma2a_core::SignedPrivateRelayAdvertisementV1>,
    expires_at_ms: u64,
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
    ) -> Result<(u64, bool), RuntimeError> {
        let authorizations = self
            .repository
            .control_spaces_for(local_endpoint_id)?
            .iter()
            .map(ma2a_store::ControlSpaceState::authorization)
            .collect::<Vec<_>>();
        let reuse = self
            .pending_relay_publication
            .as_ref()
            .is_some_and(|pending| {
                pending.config == *publisher.config()
                    && pending.authorizations == authorizations
                    && issued_at_ms < pending.expires_at_ms
            });
        if !reuse {
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
                expires_at_ms,
            });
        }
        let advertisements = &self
            .pending_relay_publication
            .as_ref()
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Control))?
            .advertisements;
        let active_spaces = advertisements
            .iter()
            .map(|advertisement| advertisement.advertisement().space_id())
            .collect::<Vec<_>>();
        let mut advanced = false;
        for advertisement in advertisements {
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
            advanced |= matches!(
                self.repository
                    .advance_private_relay_advertisement(&validated)?,
                ma2a_store::RelayAdvertisementOutcome::Advanced { .. }
            );
        }
        let (revision, activity_changed) = self
            .repository
            .reconcile_private_relay_activity(local_endpoint_id, &active_spaces)?;
        self.pending_relay_publication = None;
        Ok((revision, advanced || activity_changed))
    }
}
