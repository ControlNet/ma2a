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
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let candidates = self
            .store
            .load_relay_map(self.state.endpoint_id, now_ms)
            .await?;
        if !self.state.relay.replace_candidates(candidates.clone()) {
            return Ok(false);
        }
        self.endpoint.replace_relay_map(&candidates).await;
        self.persist_relay_observations().await?;
        self.publish_local_address_with_mode(true).await?;
        Ok(true)
    }

    pub(crate) async fn observe_iroh_relay(
        &mut self,
        observation: IrohRelayObservation,
    ) -> Result<(), RuntimeError> {
        let endpoint_changed = self.state.endpoint_data != *observation.endpoint_data();
        let relay_changed = self.state.relay.observe(&observation);
        if !endpoint_changed && !relay_changed {
            return Ok(());
        }
        self.state.endpoint_addr = observation.endpoint_addr().clone();
        self.state.endpoint_data = observation.endpoint_data().clone();
        self.persist_relay_observations().await?;
        self.state.revision = self.store.observe(&self.state).await?;
        if endpoint_changed {
            self.publish_local_address_with_mode(true).await?;
        }
        Ok(())
    }

    async fn persist_relay_observations(&mut self) -> Result<(), RuntimeError> {
        let observed_at_ms = self.clock.now_ms()?;
        let observations = self.state.relay.persisted_observations(observed_at_ms)?;
        let revision = self.store.record_relay_observations(observations).await?;
        self.state.revision = self.state.revision.max(revision);
        Ok(())
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

#[cfg(test)]
mod tests {
    use ma2a_core::{AddressEndpointDataV1, RelayReachability};
    use ma2a_net::{
        EndpointSecret, IrohHomeRelayObservation, IrohRelayObservation, LocalIrohRelayMap,
        PublicRelayFallbackConfig, RelayUrl,
    };

    use super::RelayReachabilityState;

    #[test]
    fn connected_home_outside_candidate_map_is_excluded_everywhere()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Given
        let configured: RelayUrl = "https://configured.example.invalid".parse()?;
        let outside: RelayUrl = "https://outside.example.invalid".parse()?;
        let fallback = PublicRelayFallbackConfig::new(vec![configured.clone()])?;
        let candidates = LocalIrohRelayMap::from_control_spaces(&[], Some(&fallback), 0);
        let observation = IrohRelayObservation::new(
            EndpointSecret::generate().endpoint_id(),
            AddressEndpointDataV1::new(Vec::new())?,
            vec![
                IrohHomeRelayObservation::new(configured.clone(), false),
                IrohHomeRelayObservation::new(outside, true),
            ],
        )?;
        let mut state = RelayReachabilityState::new(candidates);

        // When
        state.observe(&observation);
        let persisted = state.persisted_observations(1_000)?;

        // Then
        assert_eq!(state.reachability(), RelayReachability::NoActiveSpaces);
        assert_ne!(state.reachability(), RelayReachability::IrohHomeConnected);
        assert_eq!(
            state.observed_home_relays().collect::<Vec<_>>(),
            vec![configured.as_str()]
        );
        assert_eq!(persisted.len(), 1);
        let persisted_home = persisted
            .first()
            .ok_or("configured home was not persisted")?;
        assert_eq!(persisted_home.relay_url, configured.to_string());
        assert!(!persisted_home.reachable);
        Ok(())
    }
}
