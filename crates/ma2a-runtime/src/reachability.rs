use ma2a_core::RelayReachability;
use ma2a_net::{IrohHomeRelayObservation, IrohRelayObservation, LocalIrohRelayMap};

use crate::{
    actor::Actor,
    error::{RuntimeError, RuntimeErrorKind},
};

const RELAY_OBSERVATION_VALIDITY_MS: i64 = 60_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelayReachabilityState {
    candidates: LocalIrohRelayMap,
    reachability: RelayReachability,
    observed_home_relays: Vec<IrohHomeRelayObservation>,
}

impl RelayReachabilityState {
    pub(crate) fn new(candidates: LocalIrohRelayMap) -> Self {
        let reachability = derive_reachability(&candidates, false);
        Self {
            candidates,
            reachability,
            observed_home_relays: Vec::new(),
        }
    }

    pub(crate) const fn reachability(&self) -> RelayReachability {
        self.reachability
    }

    pub(crate) fn observed_home_relays(&self) -> impl Iterator<Item = &str> {
        self.observed_home_relays
            .iter()
            .map(|observation| observation.relay_url().as_str())
    }

    /// Every fresh private candidate with the exact Spaces it covers.
    pub(crate) fn private_coverage(
        &self,
    ) -> impl Iterator<
        Item = (
            &ma2a_net::PrivateRelayCandidate,
            &std::collections::BTreeSet<ma2a_core::SpaceId>,
        ),
    > {
        self.candidates.private_coverage()
    }

    /// Whether one candidate covers every currently active Space.
    pub(crate) fn home_relay_compatible(
        &self,
        candidate: &ma2a_net::PrivateRelayCandidate,
    ) -> bool {
        self.candidates
            .home_relay_compatible(candidate.provider_endpoint_id(), candidate.relay_url())
    }

    /// Every explicitly configured public fallback URL with its observed state.
    ///
    /// A public Iroh relay is external transport infrastructure. It has no MA2A
    /// Endpoint identity, is never a Space member, and carries no Space coverage.
    pub(crate) fn public_fallbacks(&self) -> Vec<(String, bool)> {
        self.candidates
            .public_relays()
            .iter()
            .map(|url| {
                let connected = self
                    .observed_home_relays
                    .iter()
                    .any(|home| home.is_connected() && home.relay_url() == url);
                (url.to_string(), connected)
            })
            .collect()
    }

    pub(crate) fn public_relay_online(&self) -> bool {
        self.observed_home_relays
            .iter()
            .any(|home| home.is_connected() && self.candidates.is_public_relay(home.relay_url()))
    }

    pub(crate) fn replace_candidates(&mut self, candidates: LocalIrohRelayMap) -> bool {
        if self.candidates == candidates {
            return false;
        }
        self.candidates = candidates;
        self.observed_home_relays
            .retain(|home| self.candidates.contains(home.relay_url()));
        self.reachability = derive_reachability(
            &self.candidates,
            self.observed_home_relays
                .iter()
                .any(IrohHomeRelayObservation::is_connected),
        );
        true
    }

    pub(crate) fn observe(&mut self, observation: &IrohRelayObservation) -> bool {
        let observed_home_relays = observation
            .home_relays()
            .iter()
            .filter(|home| self.candidates.contains(home.relay_url()))
            .cloned()
            .collect::<Vec<_>>();
        let reachability = derive_reachability(
            &self.candidates,
            observed_home_relays
                .iter()
                .any(IrohHomeRelayObservation::is_connected),
        );
        if self.observed_home_relays == observed_home_relays && self.reachability == reachability {
            return false;
        }
        self.observed_home_relays = observed_home_relays;
        self.reachability = reachability;
        true
    }

    pub(crate) fn persisted_observations(
        &self,
        observed_at_ms: i64,
    ) -> Result<Vec<ma2a_store::RelayObservation>, RuntimeError> {
        let expires_at_ms = observed_at_ms
            .checked_add(RELAY_OBSERVATION_VALIDITY_MS)
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Clock))?;
        Ok(self
            .observed_home_relays
            .iter()
            .map(|home| ma2a_store::RelayObservation {
                relay_url: home.relay_url().to_string(),
                observed_at_ms,
                expires_at_ms,
                reachable: home.is_connected(),
                latency_ms: None,
                observed_state: vec![u8::from(home.is_connected())],
            })
            .collect())
    }
}

impl Actor {
    pub(crate) async fn refresh_relay_candidates(&mut self) -> Result<bool, RuntimeError> {
        #[cfg(test)]
        {
            self.maintenance
                .candidate_refresh_attempts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.maintenance
                .check(crate::actor::maintenance::FaultPoint::RelayCandidateLoad)?;
        }
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let candidates = self
            .store
            .load_relay_map(self.state.endpoint_id, now_ms)
            .await?;
        let mut staged = self.state.relay.clone();
        if !staged.replace_candidates(candidates.clone()) {
            return Ok(false);
        }
        #[cfg(test)]
        {
            self.maintenance.relay_map_apply_attempts += 1;
            self.maintenance
                .check(crate::actor::maintenance::FaultPoint::RelayMapApply)?;
        }
        self.endpoint.replace_relay_map(&candidates).await?;
        let before_revision = self.state.revision;
        let observed_revision = self.persist_relay_observations(&staged).await?;
        let address_result = self.publish_local_address_with_mode(false).await;
        self.state.revision = before_revision;
        let address_revision = address_result?;
        let mut revision = observed_revision.max(address_revision);
        if revision <= before_revision {
            revision = self.store.advance_revision().await?;
        }
        self.state.relay = staged;
        self.state.revision = revision;
        Ok(true)
    }

    pub(crate) async fn observe_iroh_relay(
        &mut self,
        observation: IrohRelayObservation,
    ) -> Result<(), RuntimeError> {
        // Iroh sends complete snapshots. Keep the latest desired snapshot until
        // every durable projection is reconciled, even if no event is repeated.
        self.maintenance.pending_observation = Some(observation);
        self.reconcile_pending_iroh_observation().await
    }

    pub(crate) async fn reconcile_pending_iroh_observation(&mut self) -> Result<(), RuntimeError> {
        let Some(observation) = self.maintenance.pending_observation.as_ref() else {
            return Ok(());
        };
        let mut staged = self.state.clone();
        staged.direct_reachable = observation.endpoint_addr().ip_addrs().next().is_some();
        let endpoint_changed = staged.endpoint_data != *observation.endpoint_data();
        let relay_changed = staged.relay.observe(observation);
        staged.endpoint_addr = observation.endpoint_addr().clone();
        staged.endpoint_data = observation.endpoint_data().clone();
        if endpoint_changed || relay_changed {
            let before_revision = self.state.revision;
            let relay = staged.relay.clone();
            let observed_revision = self.persist_relay_observations(&relay).await?;
            #[cfg(test)]
            self.maintenance
                .check(crate::actor::maintenance::FaultPoint::EndpointObservationPersist)?;
            let metadata_revision = self.store.observe_if_changed(&staged).await?;
            let mut revision = observed_revision.max(metadata_revision);
            if endpoint_changed {
                let address_result = self.publish_local_address_with_mode(false).await;
                self.state.revision = before_revision;
                revision = revision.max(address_result?);
            }
            if revision <= before_revision {
                revision = self.store.advance_revision().await?;
            }
            staged.revision = revision;
        }
        self.state = staged;
        self.maintenance.pending_observation = None;
        Ok(())
    }

    async fn persist_relay_observations(
        &self,
        relay: &RelayReachabilityState,
    ) -> Result<u64, RuntimeError> {
        let observed_at_ms = self.clock.now_ms()?;
        let observations = relay.persisted_observations(observed_at_ms)?;
        #[cfg(test)]
        self.maintenance
            .check(crate::actor::maintenance::FaultPoint::RelayObservationPersist)?;
        let revision = self.store.record_relay_observations(observations).await?;
        Ok(revision)
    }
}

fn derive_reachability(
    candidates: &LocalIrohRelayMap,
    connected_observed_home: bool,
) -> RelayReachability {
    if candidates.active_space_count() == 0 {
        RelayReachability::NoActiveSpaces
    } else if candidates.is_empty() {
        RelayReachability::DegradedNoCommonHome
    } else if connected_observed_home {
        RelayReachability::IrohHomeConnected
    } else {
        RelayReachability::AwaitingIrohHome
    }
}
