use super::StoreBackend;
use crate::error::RuntimeError;

impl StoreBackend {
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
