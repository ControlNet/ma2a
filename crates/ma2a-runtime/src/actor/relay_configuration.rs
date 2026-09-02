use ma2a_net::{PrivateRelayAccess, PrivateRelayServer, RuntimeRelayConfiguration};

use super::Actor;
use crate::error::{RuntimeError, RuntimeErrorKind};

impl Actor {
    pub(super) async fn relay_runtime_status(
        &self,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let configuration = self.store.relay_configuration().await?;
        Ok(super::RelayRuntimeStatus {
            configuration,
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
    ) -> Result<u64, RuntimeError> {
        let runtime = RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let replacement = match runtime.private_provider() {
            Some(provider) => {
                let access =
                    PrivateRelayAccess::new(self.state.endpoint_id, provider.served_spaces());
                let authorizations = self
                    .store
                    .relay_authorizations(self.state.endpoint_id)
                    .await?;
                access.replace_from_spaces(&authorizations);
                Some(
                    PrivateRelayServer::spawn(provider, access)
                        .await
                        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
                )
            }
            None => None,
        };
        let revision = self.store.set_relay_configuration(configuration).await?;
        if let Some(server) = self.private_relay_server.take() {
            server
                .shutdown()
                .await
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Shutdown))?;
        }
        self.private_relay_server = replacement;
        self.state.revision = revision;
        self.refresh_local_relay_publication().await?;
        self.refresh_relay_candidates().await?;
        let _receiver_count = self
            .events
            .send(crate::state::RuntimeEvent::memberships_changed(
                self.state.revision,
            ));
        Ok(self.state.revision)
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
