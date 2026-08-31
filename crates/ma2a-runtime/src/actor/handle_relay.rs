use tokio::sync::oneshot;

use crate::error::{RuntimeError, RuntimeErrorKind};

use super::{Command, RuntimeHandle};

#[derive(Clone, Debug)]
pub(crate) struct RelayRuntimeStatus {
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

    pub(crate) async fn set_relay_configuration(
        &self,
        configuration: ma2a_store::RelayConfiguration,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SetRelayConfiguration {
                configuration,
                reply,
            })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }
}
