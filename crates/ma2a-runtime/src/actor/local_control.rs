use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};

impl Actor {
    pub(crate) async fn refresh_local_control_publications(&mut self) -> Result<(), RuntimeError> {
        let address = self.publish_local_address().await.map(|_| ());
        let relay = self.refresh_local_relay_publication().await;
        match (address, relay) {
            (Err(first), Err(second))
                if first.is_retryable_background() && !second.is_retryable_background() =>
            {
                Err(second)
            }
            (first, second) => first.and(second),
        }
    }

    pub(crate) async fn refresh_local_relay_publication(&mut self) -> Result<(), RuntimeError> {
        let configuration = self.store.relay_configuration().await?;
        let runtime = ma2a_net::RuntimeRelayConfiguration::try_from(configuration)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        if let Some(provider) = runtime.private_provider() {
            if self
                .private_relay_server
                .as_ref()
                .map(ma2a_net::PrivateRelayServer::configuration)
                != Some(provider)
            {
                // Desired provider state is not publishable until its server is applied.
                // Server reconciliation owns this pending work and retries on maintenance.
                return Ok(());
            }
            let issued_at_ms = u64::try_from(self.clock.now_ms()?)
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
            let expires_at_ms = issued_at_ms
                .checked_add(ma2a_core::MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS)
                .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Clock))?;
            // One clock reading gives the renewal both its issue time and its expiry, so a
            // clock step between two readings cannot leave it an invalid window. Always reconcile
            // durable activity against `effective`, skipping signing only for current Spaces.
            self.publish_local_relay(provider.clone(), (issued_at_ms, expires_at_ms, true))
                .await?;
        } else {
            self.maintenance.published_relay.clear();
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

    pub(crate) async fn publish_local_address(&mut self) -> Result<u64, RuntimeError> {
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        #[cfg(test)]
        self.maintenance
            .check(super::maintenance::FaultPoint::AddressObservation)?;
        let publisher = self
            .endpoint
            .address_publisher()
            .map_err(RuntimeError::from)?;
        let write_result = self
            .store
            .publish_address(publisher, self.state.endpoint_id, now_ms)
            .await;
        if write_result.is_err() {
            // The Store may have committed some Space records before failing.
            self.maintenance.address_lookup_pending = true;
        }
        let outcome = write_result?;
        self.state.revision = self.state.revision.max(outcome.revision);
        if outcome.advanced {
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
        if outcome.clock_behind {
            return Err(RuntimeError::new(RuntimeErrorKind::ClockBehind));
        }
        Ok(outcome.revision)
    }

    /// Publishes advertisements issued now that expire at `expires_at_ms`.
    pub(super) async fn publish_local_relay_until(
        &mut self,
        config: ma2a_net::PrivateRelayProviderConfig,
        expires_at_ms: u64,
    ) -> Result<u64, RuntimeError> {
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        ma2a_core::PrivateRelayAdvertisementValidity::new(1, issued_at_ms, expires_at_ms)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        self.publish_local_relay(config, (issued_at_ms, expires_at_ms, false))
            .await
    }

    async fn publish_local_relay(
        &mut self,
        config: ma2a_net::PrivateRelayProviderConfig,
        (issued_at_ms, expires_at_ms, use_freshness_cache): (u64, u64, bool),
    ) -> Result<u64, RuntimeError> {
        let authorizations = self
            .store
            .relay_authorizations(self.state.endpoint_id)
            .await?;
        let effective = self.effective_relay_spaces(&config, &authorizations);
        let skip_spaces = if use_freshness_cache {
            self.current_relay_spaces(
                (config.public_relay_url().as_str(), issued_at_ms),
                &effective,
            )
        } else {
            std::collections::BTreeSet::new()
        };
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
                skip_spaces.clone(),
            )
            .await;
        if write_result.is_err() {
            // A reserved sequence or a subset of advertisements may already
            // be durable. The Store retries that exact signed batch.
            self.maintenance.relay_followup_pending = true;
        }
        let publication_result = write_result?;
        self.state.revision = self.state.revision.max(publication_result.revision);
        self.maintenance
            .published_relay
            .retain(|space_id, _| skip_spaces.contains(space_id));
        let relay_url = config.public_relay_url().as_str();
        for space_id in publication_result.published_spaces {
            self.maintenance.published_relay.insert(
                space_id,
                super::maintenance::PublishedRelaySpace {
                    relay_url: relay_url.to_owned(),
                    issued_at_ms: publication_result.issued_at_ms,
                    expires_at_ms: publication_result.expires_at_ms,
                },
            );
        }
        if publication_result.changed {
            self.maintenance.relay_followup_pending = true;
        }
        self.finish_relay_publication().await?;
        if publication_result.clock_behind {
            return Err(RuntimeError::new(RuntimeErrorKind::ClockBehind));
        }
        Ok(publication_result.revision)
    }

    fn effective_relay_spaces(
        &self,
        config: &ma2a_net::PrivateRelayProviderConfig,
        authorizations: &[ma2a_core::SpaceAuthorizationView],
    ) -> std::collections::BTreeSet<ma2a_core::SpaceId> {
        authorizations
            .iter()
            .filter(|authorization| {
                config.served_spaces().contains(&authorization.space_id())
                    && authorization.allows_private_relay_provider(self.state.endpoint_id)
            })
            .map(ma2a_core::SpaceAuthorizationView::space_id)
            .collect()
    }

    fn current_relay_spaces(
        &self,
        (relay_url, issued_at_ms): (&str, u64),
        effective: &std::collections::BTreeSet<ma2a_core::SpaceId>,
    ) -> std::collections::BTreeSet<ma2a_core::SpaceId> {
        self.maintenance
            .published_relay
            .iter()
            .filter(|(space_id, published)| {
                effective.contains(space_id)
                    && published.relay_url == relay_url
                    && crate::relay_publication::is_current(
                        issued_at_ms,
                        published.issued_at_ms,
                        published.expires_at_ms,
                    )
            })
            .map(|(space_id, _)| *space_id)
            .collect()
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
