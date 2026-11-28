//! Durable relay configuration is desired state; the server owns its applied configuration.
use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};
use ma2a_net::{
    PrivateRelayAccess, PrivateRelayProviderConfig, PrivateRelayServer, RuntimeRelayConfiguration,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PrivateRelayApply {
    #[default]
    IfChanged,
    Reload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayCompletion {
    Pending(PrivateRelayApply),
    ShutdownFailed,
}

impl Actor {
    pub(super) async fn relay_runtime_status(
        &self,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let desired = self.store.relay_configuration_committed().await?;
        let revision = desired.revision();
        self.relay_status_for(desired.into_value(), revision)
    }

    pub(super) fn relay_status_for(
        &self,
        configuration: ma2a_store::RelayConfiguration,
        revision: u64,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let desired = RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        let convergence_pending =
            self.maintenance.relay_configuration.is_some() || applied != desired.private_provider();
        Ok(super::RelayRuntimeStatus {
            revision,
            configuration,
            applied_private: self
                .private_relay_server
                .as_ref()
                .map(|server| server.configuration().clone()),
            convergence_pending,
            private_listen_addr: self
                .private_relay_server
                .as_ref()
                .map(PrivateRelayServer::listen_addr),
            public_relay_online: self.state.relay.public_relay_online(),
        })
    }

    pub(super) async fn apply_private_relay(
        &mut self,
        desired: Option<&PrivateRelayProviderConfig>,
        (mode, preloaded_tls): (PrivateRelayApply, Option<ma2a_net::LoadedRelayTls>),
    ) -> Result<(), RuntimeError> {
        let was_running = self.private_relay_server.is_some();
        let result = self
            .apply_private_relay_inner(desired, (mode, preloaded_tls))
            .await;
        if self.private_relay_server.is_some() != was_running {
            self.view_sequence = self.view_sequence.saturating_add(1);
        }
        result
    }

    async fn apply_private_relay_inner(
        &mut self,
        desired: Option<&PrivateRelayProviderConfig>,
        (mode, preloaded_tls): (PrivateRelayApply, Option<ma2a_net::LoadedRelayTls>),
    ) -> Result<(), RuntimeError> {
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        if applied == desired && mode == PrivateRelayApply::IfChanged {
            return Ok(());
        }
        // A conflicting listener must release the current socket first.
        // Non-conflicting listeners can prepare the replacement while the old server serves.
        let conflicts = self
            .private_relay_server
            .as_ref()
            .zip(desired)
            .is_some_and(|(old, new)| listeners_conflict(old.listen_addr(), new.listen_addr()));
        if conflicts || desired.is_none() {
            self.stop_private_relay().await?;
        }
        let replacement = match desired {
            Some(provider) => Some(self.spawn_private_relay(provider, preloaded_tls).await?),
            None => None,
        };
        if let Err(error) = self.stop_private_relay().await {
            if let Some(server) = replacement {
                let _shutdown = server.shutdown().await;
            }
            return Err(error);
        }
        self.private_relay_server = replacement;
        Ok(())
    }

    async fn spawn_private_relay(
        &self,
        provider: &PrivateRelayProviderConfig,
        preloaded_tls: Option<ma2a_net::LoadedRelayTls>,
    ) -> Result<PrivateRelayServer, RuntimeError> {
        #[cfg(test)]
        self.maintenance
            .check(super::maintenance::FaultPoint::PrivateRelayStart)?;
        let access = PrivateRelayAccess::new(self.state.endpoint_id, provider.served_spaces());
        access.replace_from_spaces(
            &self
                .store
                .relay_authorizations(self.state.endpoint_id)
                .await?,
        );
        PrivateRelayServer::spawn_with_tls(provider, access, preloaded_tls)
            .await
            .map_err(|error| RuntimeError::new(RuntimeErrorKind::PrivateRelay(error)))
    }

    async fn stop_private_relay(&mut self) -> Result<(), RuntimeError> {
        let Some(server) = self.private_relay_server.take() else {
            return Ok(());
        };
        let stopped = server
            .shutdown()
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Shutdown));
        #[cfg(test)]
        let stopped = stopped.and_then(|()| {
            self.maintenance
                .check(super::maintenance::FaultPoint::PrivateRelayShutdown)
        });
        if stopped.is_err() {
            self.maintenance.relay_configuration = Some(RelayCompletion::ShutdownFailed);
        }
        stopped
    }
}

fn listeners_conflict(bound: std::net::SocketAddr, desired: std::net::SocketAddr) -> bool {
    desired.port() != 0
        && bound.port() == desired.port()
        && (bound.ip().to_canonical() == desired.ip().to_canonical()
            || bound.ip().is_unspecified()
            || desired.ip().is_unspecified())
}

#[cfg(test)]
mod tests {
    use super::listeners_conflict;

    #[test]
    fn listeners_conflict_detects_bound_ephemeral_and_wildcard_overlaps() {
        let addr = |text: &str| text.parse::<std::net::SocketAddr>().expect("valid socket");
        assert!(listeners_conflict(
            addr("127.0.0.1:8443"),
            addr("127.0.0.1:8443")
        ));
        assert!(listeners_conflict(
            addr("0.0.0.0:8443"),
            addr("127.0.0.1:8443")
        ));
        assert!(listeners_conflict(
            addr("127.0.0.1:8443"),
            addr("0.0.0.0:8443")
        ));
        assert!(listeners_conflict(
            addr("[::]:8443"),
            addr("127.0.0.1:8443")
        ));
        assert!(listeners_conflict(addr("[::]:8443"), addr("0.0.0.0:8443")));
        assert!(listeners_conflict(
            addr("[::ffff:127.0.0.1]:8443"),
            addr("127.0.0.1:8443")
        ));
        assert!(!listeners_conflict(
            addr("127.0.0.1:8443"),
            addr("127.0.0.1:0")
        ));
        assert!(!listeners_conflict(
            addr("127.0.0.1:8443"),
            addr("127.0.0.2:8443")
        ));
        assert!(!listeners_conflict(
            addr("0.0.0.0:8443"),
            addr("127.0.0.1:8444")
        ));
    }
}
