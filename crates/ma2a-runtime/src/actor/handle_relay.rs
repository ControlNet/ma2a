use tokio::sync::oneshot;

use crate::error::{RuntimeError, RuntimeErrorKind};

use super::{Command, RuntimeHandle};

#[derive(Clone, Debug)]
pub(crate) struct RelayRuntimeStatus {
    pub(crate) revision: u64,
    pub(crate) applied_private: Option<ma2a_net::PrivateRelayProviderConfig>,
    pub(crate) convergence_pending: bool,
    pub(crate) configuration: ma2a_store::RelayConfiguration,
    pub(crate) private_listen_addr: Option<std::net::SocketAddr>,
    pub(crate) public_relay_online: bool,
}

impl RuntimeHandle {
    pub(crate) async fn relay_configuration(
        &self,
    ) -> Result<ma2a_store::RelayConfiguration, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RelayConfiguration { reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    pub(crate) async fn relay_status(&self) -> Result<RelayRuntimeStatus, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RelayStatus { reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    #[cfg(test)]
    pub(crate) async fn set_relay_configuration(
        &self,
        configuration: ma2a_store::RelayConfiguration,
    ) -> Result<u64, RuntimeError> {
        Ok(self
            .set_relay_configuration_committed(configuration)
            .await?
            .revision())
    }

    pub(crate) async fn set_relay_configuration_committed(
        &self,
        configuration: ma2a_store::RelayConfiguration,
    ) -> Result<ma2a_store::Committed<RelayRuntimeStatus>, RuntimeError> {
        self.apply_relay_configuration(
            configuration,
            super::relay_server::PrivateRelayApply::IfChanged,
        )
        .await
    }

    pub(crate) async fn reconfigure_private_relay(
        &self,
        configuration: ma2a_store::RelayConfiguration,
    ) -> Result<ma2a_store::Committed<RelayRuntimeStatus>, RuntimeError> {
        self.apply_relay_configuration(
            configuration,
            super::relay_server::PrivateRelayApply::Reload,
        )
        .await
    }

    async fn apply_relay_configuration(
        &self,
        configuration: ma2a_store::RelayConfiguration,
        mode: super::relay_server::PrivateRelayApply,
    ) -> Result<ma2a_store::Committed<RelayRuntimeStatus>, RuntimeError> {
        let runtime = ma2a_net::RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        // Load and validate native TLS files on the blocking pool before entering the Actor,
        // so slow filesystems never stall status, snapshot, or peer protocols.
        let preloaded_tls = if let Some(provider) = runtime.private_provider()
            && let ma2a_net::PrivateRelayTransport::NativeTls(tls) = provider.transport()
        {
            let tls = tls.clone();
            let loaded = tokio::task::spawn_blocking(move || {
                super::relay_configuration::load_native_tls_blocking(&tls)
            })
            .await
            .map_err(RuntimeError::from)??;
            Some(loaded)
        } else {
            None
        };
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SetRelayConfiguration {
                configuration,
                mode,
                preloaded_tls,
                reply,
            })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }
}
