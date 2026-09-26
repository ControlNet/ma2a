//! Durable relay configuration is desired state; the server owns its applied configuration.
use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};
use ma2a_net::{PrivateRelayAccess, PrivateRelayProviderConfig, PrivateRelayServer};

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
    pub(super) async fn apply_private_relay(
        &mut self,
        desired: Option<&PrivateRelayProviderConfig>,
        mode: PrivateRelayApply,
    ) -> Result<(), RuntimeError> {
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        if applied == desired && mode == PrivateRelayApply::IfChanged {
            return Ok(());
        }
        // A same-listener replacement must release the current listener first.
        // Different listeners can prepare the replacement while the old server serves.
        let same_listener = applied
            .zip(desired)
            .is_some_and(|(old, new)| old.listen_addr() == new.listen_addr());
        if same_listener || desired.is_none() {
            self.stop_private_relay().await?;
        }
        let replacement = match desired {
            Some(provider) => Some(self.spawn_private_relay(provider).await?),
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
        PrivateRelayServer::spawn(provider, access)
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
