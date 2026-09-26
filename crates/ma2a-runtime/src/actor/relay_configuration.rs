use ma2a_net::{PrivateRelayServer, RuntimeRelayConfiguration};

use super::{
    Actor,
    relay_server::{PrivateRelayApply, RelayCompletion},
};
use crate::error::{RuntimeError, RuntimeErrorKind};

impl Actor {
    pub(super) async fn relay_runtime_status(
        &self,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let desired = self.store.relay_configuration_committed().await?;
        let revision = desired.revision();
        self.relay_status_for(desired.into_value(), revision)
    }

    fn relay_status_for(
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

    pub(super) async fn set_relay_configuration(
        &mut self,
        configuration: ma2a_store::RelayConfiguration,
        mode: PrivateRelayApply,
    ) -> Result<ma2a_store::Committed<super::RelayRuntimeStatus>, RuntimeError> {
        let runtime = RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        // Reject bad TLS input before committing desired state or touching the old listener.
        if let Some(provider) = runtime.private_provider()
            && let ma2a_net::PrivateRelayTransport::NativeTls(tls) = provider.transport()
        {
            tls.load_server_config()
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        }
        let revision = self
            .store
            .set_relay_configuration(configuration.clone())
            .await?;
        self.state.revision = self.state.revision.max(revision);
        self.maintenance.relay_configuration = Some(RelayCompletion::Pending(mode));
        self.reconcile_relay_configuration()
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(ma2a_store::Committed::new(
            revision,
            self.relay_status_for(configuration, revision)
                .map_err(|error| error.after_commit(revision))?,
        ))
    }

    pub(crate) async fn reconcile_relay_configuration(&mut self) -> Result<(), RuntimeError> {
        let desired = self.store.relay_configuration().await?;
        let runtime = RuntimeRelayConfiguration::try_from(desired)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        let mode = match self.maintenance.relay_configuration {
            Some(RelayCompletion::ShutdownFailed) => {
                return Err(RuntimeError::new(RuntimeErrorKind::Shutdown));
            }
            Some(RelayCompletion::Pending(mode)) => mode,
            None if applied == runtime.private_provider() => return Ok(()),
            None => PrivateRelayApply::IfChanged,
        };
        self.maintenance.relay_configuration = Some(RelayCompletion::Pending(mode));
        self.apply_private_relay(runtime.private_provider(), mode)
            .await?;
        // Once a server is installed, later publication failures must not restart it again.
        self.maintenance.relay_configuration =
            Some(RelayCompletion::Pending(PrivateRelayApply::IfChanged));
        self.refresh_private_relay_access().await?;
        self.refresh_local_relay_publication().await?;
        self.refresh_relay_candidates().await?;
        self.maintenance.relay_configuration = None;
        let _receiver_count = self
            .events
            .send(crate::state::RuntimeEvent::memberships_changed(
                self.state.revision,
            ));
        Ok(())
    }

    pub(crate) async fn refresh_private_relay_access(&self) -> Result<(), RuntimeError> {
        if let Some(server) = &self.private_relay_server {
            let authorizations = self
                .store
                .relay_authorizations(self.state.endpoint_id)
                .await?;
            server.replace_from_spaces(&authorizations);
        }
        Ok(())
    }
}
