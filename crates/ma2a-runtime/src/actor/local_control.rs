use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};

impl Actor {
    pub(super) async fn publish_local_address(&mut self) -> Result<u64, RuntimeError> {
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let publisher = self
            .endpoint
            .address_publisher()
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let (revision, advanced) = self
            .store
            .publish_address(publisher, self.state.endpoint_id, now_ms)
            .await?;
        self.state.revision = revision;
        if advanced {
            self.refresh_control_lookup().await?;
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::AddressAdvanced,
                None,
            );
        }
        Ok(revision)
    }

    pub(super) async fn publish_local_relay(
        &mut self,
        config: ma2a_net::PrivateRelayProviderConfig,
        expires_at_ms: u64,
    ) -> Result<u64, RuntimeError> {
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let publisher = self.endpoint.private_relay_advertisement_publisher(config);
        let (revision, advanced) = self
            .store
            .publish_relay_advertisements(
                publisher,
                self.state.endpoint_id,
                issued_at_ms,
                expires_at_ms,
            )
            .await?;
        self.state.revision = revision;
        if advanced {
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::RelayAdvanced,
                None,
            );
        }
        Ok(revision)
    }
}
