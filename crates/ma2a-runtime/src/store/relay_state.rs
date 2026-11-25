use super::StoreBackend;
use crate::error::RuntimeError;
use tokio::sync::oneshot;

impl StoreBackend {
    pub(super) fn reply_relay_configuration(
        &self,
        reply: oneshot::Sender<
            Result<ma2a_store::Committed<ma2a_store::RelayConfiguration>, RuntimeError>,
        >,
    ) {
        let _unsent = reply.send(
            self.repository
                .relay_configuration_committed()
                .map_err(Into::into),
        );
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

    /// Switches off each part of the stored relay configuration this Runtime cannot use, such
    /// as one whose URL a later version refuses, so that it cannot stop the Runtime from
    /// starting, and saves the URLs it keeps as parsed. The operator configures a switched-off
    /// relay again.
    pub(super) fn drop_unusable_relay_configuration(&mut self) -> Result<(), RuntimeError> {
        let stored = self.repository.relay_configuration()?;
        let usable = usable_relay_configuration(&stored);
        if usable.public_fallback_enabled != stored.public_fallback_enabled {
            eprintln!("stored public relay fallback cannot be used; it is switched off");
        }
        if usable.private_provider_enabled != stored.private_provider_enabled {
            eprintln!("stored private relay provider cannot be used; it is switched off");
        }
        if usable != stored {
            self.repository.set_relay_configuration(&usable)?;
        }
        Ok(())
    }
}

/// `stored` with each of its two independent parts, the public fallback and the private
/// provider, kept only if the Runtime can use it.
fn usable_relay_configuration(
    stored: &ma2a_store::RelayConfiguration,
) -> ma2a_store::RelayConfiguration {
    let usable = |configuration: &ma2a_store::RelayConfiguration| {
        ma2a_net::RuntimeRelayConfiguration::try_from(configuration.clone()).is_ok()
    };
    let mut kept = stored.clone();
    let public_only = ma2a_store::RelayConfiguration {
        private_provider_enabled: false,
        listener_address: None,
        private_relay_url: None,
        served_spaces: Vec::new(),
        transport: None,
        ..stored.clone()
    };
    if !usable(&public_only) {
        kept.public_fallback_enabled = false;
        kept.public_relay_urls.clear();
    }
    let private_only = ma2a_store::RelayConfiguration {
        public_fallback_enabled: false,
        public_relay_urls: Vec::new(),
        ..stored.clone()
    };
    if !usable(&private_only) {
        kept.private_provider_enabled = false;
        kept.listener_address = None;
        kept.private_relay_url = None;
        kept.served_spaces.clear();
        kept.transport = None;
    }
    // A kept URL is saved as parsed, the spelling the Runtime uses and the Store bounds; text
    // written before the bound existed may be longer than its parsed form.
    let parsed = |url: &String| {
        url.parse::<ma2a_net::RelayUrl>()
            .map_or_else(|_| url.clone(), |parsed| parsed.as_str().to_owned())
    };
    kept.public_relay_urls = kept.public_relay_urls.iter().map(parsed).collect();
    kept.private_relay_url = kept.private_relay_url.as_ref().map(parsed);
    kept
}

#[cfg(test)]
#[path = "relay_state_tests.rs"]
mod tests;
