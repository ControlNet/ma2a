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
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SetRelayConfiguration {
                configuration,
                mode,
                reply,
            })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }
}
