use super::StoreBackend;
use crate::error::{RuntimeError, RuntimeErrorKind};

impl StoreBackend {
    #[expect(
        clippy::too_many_arguments,
        reason = "publication inputs map directly to the signed record"
    )]
    pub(super) fn publish_address(
        &mut self,
        publisher: &ma2a_net::AddressPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
    ) -> Result<(u64, bool), RuntimeError> {
        let states = self.repository.control_spaces_for(local_endpoint_id)?;
        let mut advanced = false;
        for state in states {
            advanced |= publisher
                .publish(
                    &mut self.repository,
                    ma2a_net::AddressPublishRequest::new(&state.authorization(), now_ms),
                )
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?
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
        let advertisements = publisher
            .advertisements(
                &mut self.repository,
                ma2a_net::AdvertisementPublicationRequest::new(
                    &authorizations,
                    issued_at_ms,
                    expires_at_ms,
                ),
            )
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
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
        Ok((self.repository.revision()?, advanced))
    }
}
