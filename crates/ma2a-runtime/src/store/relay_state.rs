use super::StoreBackend;
use crate::error::RuntimeError;
use tokio::sync::oneshot;

impl StoreBackend {
    pub(super) fn reply_relay_configuration(
        &self,
        reply: oneshot::Sender<Result<ma2a_store::RelayConfiguration, RuntimeError>>,
    ) {
        let _unsent = reply.send(self.repository.relay_configuration().map_err(Into::into));
    }

    pub(super) fn reply_set_relay_configuration(
        &mut self,
        configuration: &ma2a_store::RelayConfiguration,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    ) {
        let _unsent = reply.send(
            self.repository
                .set_relay_configuration(configuration)
                .map_err(Into::into),
        );
    }

    pub(super) fn reply_relay_authorizations(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
        reply: oneshot::Sender<Result<Vec<ma2a_core::SpaceAuthorizationView>, RuntimeError>>,
    ) {
        let result = self
            .repository
            .control_spaces_for(local_endpoint_id)
            .map(|spaces| {
                spaces
                    .iter()
                    .map(ma2a_store::ControlSpaceState::authorization)
                    .collect()
            })
            .map_err(Into::into);
        let _unsent = reply.send(result);
    }

    pub(super) fn load_relay_map(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
    ) -> Result<ma2a_net::LocalIrohRelayMap, RuntimeError> {
        let persisted = self.repository.relay_configuration()?;
        let runtime = ma2a_net::RuntimeRelayConfiguration::try_from(persisted).map_err(|_| {
            ma2a_store::StoreError::SchemaMismatch {
                detail: "persisted relay configuration is invalid",
            }
        })?;
        let spaces = self.repository.control_spaces_for(local_endpoint_id)?;
        Ok(ma2a_net::LocalIrohRelayMap::from_control_spaces(
            &spaces,
            runtime.public_fallback(),
            now_ms,
        ))
    }

    pub(super) fn record_relay_observations(
        &mut self,
        observations: &[ma2a_store::RelayObservation],
    ) -> Result<u64, RuntimeError> {
        self.repository
            .replace_relay_observations(observations)
            .map_err(Into::into)
    }
}
