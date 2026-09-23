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
            let authorizations = self
                .store
                .relay_authorizations(self.state.endpoint_id)
                .await?;
            if self
                .maintenance
                .published_relay
                .as_ref()
                .is_some_and(|published| issued_at_ms < published.issued_at_ms)
            {
                return Err(RuntimeError::new(RuntimeErrorKind::Clock));
            }
            let current = self
                .maintenance
                .published_relay
                .as_ref()
                .is_some_and(|published| {
                    published.config == *provider
                        && published.authorizations == authorizations
                        && issued_at_ms >= published.issued_at_ms
                        && issued_at_ms - published.issued_at_ms < 300_000
                        && issued_at_ms.saturating_add(300_000) < published.expires_at_ms
                });
            if current {
                self.finish_relay_publication().await?;
            } else {
                self.publish_local_relay(provider.clone(), expires_at_ms)
                    .await?;
            }
        } else {
            self.maintenance.published_relay = None;
            let reconciled = self
                .store
                .reconcile_relay_activity(self.state.endpoint_id, Vec::new())
                .await;
            if reconciled.is_err() {
                self.maintenance.relay_followup_pending = true;
            }
            let (revision, changed) = reconciled?;
            self.state.revision = self.state.revision.max(revision);
            if changed {
                self.maintenance.relay_followup_pending = true;
            }
            self.finish_relay_publication().await?;
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
            .map_err(RuntimeError::from)?;
        let write_result = self
            .store
            .publish_address(publisher, self.state.endpoint_id, now_ms, force_advance)
            .await;
        if write_result.is_err() {
            // The Store may have committed some Space records before failing.
            self.maintenance.address_lookup_pending = true;
        }
        let (revision, advanced) = write_result?;
        self.state.revision = self.state.revision.max(revision);
        if advanced {
            self.maintenance.address_lookup_pending = true;
        }
        if self.maintenance.address_lookup_pending {
            #[cfg(test)]
            self.maintenance
                .check(super::maintenance::FaultPoint::AddressPublication)?;
            self.refresh_control_lookup().await?;
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::AddressAdvanced,
                None,
            );
            self.maintenance.address_lookup_pending = false;
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
        let authorizations = self
            .store
            .relay_authorizations(self.state.endpoint_id)
            .await?;
        let publisher = self
            .endpoint
            .private_relay_advertisement_publisher(config.clone());
        let write_result = self
            .store
            .publish_relay_advertisements(
                publisher,
                self.state.endpoint_id,
                issued_at_ms,
                expires_at_ms,
            )
            .await;
        if write_result.is_err() {
            // A reserved sequence or a subset of advertisements may already
            // be durable. The Store retries that exact signed batch.
            self.maintenance.relay_followup_pending = true;
        }
        let (revision, advanced) = write_result?;
        self.state.revision = self.state.revision.max(revision);
        self.maintenance.published_relay = Some(super::maintenance::PublishedRelay {
            config,
            authorizations,
            issued_at_ms,
            expires_at_ms,
        });
        if advanced {
            self.maintenance.relay_followup_pending = true;
        }
        self.finish_relay_publication().await?;
        Ok(revision)
    }

    async fn finish_relay_publication(&mut self) -> Result<(), RuntimeError> {
        if self.maintenance.relay_followup_pending {
            #[cfg(test)]
            self.maintenance
                .check(super::maintenance::FaultPoint::RelayPublication)?;
            self.refresh_relay_candidates().await?;
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::RelayAdvanced,
                None,
            );
            self.maintenance.relay_followup_pending = false;
        }
        Ok(())
    }
}
