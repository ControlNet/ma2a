use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};

impl Actor {
    pub(crate) async fn refresh_local_control_publications(&mut self) -> Result<(), RuntimeError> {
        self.publish_local_address_with_mode(false).await?;
        self.refresh_local_relay_publication().await?;
        Ok(())
    }

    pub(crate) async fn refresh_local_relay_publication(&mut self) -> Result<(), RuntimeError> {
        let configuration = self.store.relay_configuration().await?;
        let runtime = ma2a_net::RuntimeRelayConfiguration::try_from(configuration)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        if let Some(provider) = runtime.private_provider() {
            let issued_at_ms = u64::try_from(self.clock.now_ms()?)
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
            let expires_at_ms = issued_at_ms
                .checked_add(ma2a_core::MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS)
                .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Clock))?;
            self.publish_local_relay(provider.clone(), expires_at_ms)
                .await?;
        } else {
            let (revision, changed) = self
                .store
                .reconcile_relay_activity(self.state.endpoint_id, Vec::new())
                .await?;
            self.state.revision = revision;
            if changed {
                self.refresh_relay_candidates().await?;
                self.schedule_control_round(
                    crate::control_sync::ControlRoundTrigger::RelayAdvanced,
                    None,
                );
            }
        }
        Ok(())
    }

    pub(super) async fn publish_local_address(&mut self) -> Result<u64, RuntimeError> {
        self.publish_local_address_with_mode(false).await
    }

    pub(crate) async fn publish_local_address_with_mode(
        &mut self,
        force_advance: bool,
    ) -> Result<u64, RuntimeError> {
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let publisher = self
            .endpoint
            .address_publisher()
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let (revision, advanced) = self
            .store
            .publish_address(publisher, self.state.endpoint_id, now_ms, force_advance)
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
            self.refresh_relay_candidates().await?;
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::RelayAdvanced,
                None,
            );
        }
        Ok(revision)
    }
}
