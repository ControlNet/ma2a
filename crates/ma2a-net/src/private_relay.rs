use std::{error::Error, fmt, net::SocketAddr, sync::Arc};

use iroh_relay::server::{
    CertConfig, RelayConfig, Server, ServerConfig, TlsConfig, clients::Clients,
};
use ma2a_core::SpaceAuthorizationView;

use crate::{PrivateRelayAccess, PrivateRelayProviderConfig, PrivateRelayTransport, RelayTlsError};

/// Running private Iroh relay hosted by the Runtime process.
#[derive(Debug)]
pub struct PrivateRelayServer {
    server: Server,
    access: PrivateRelayAccess,
    clients: Clients,
    listen_addr: SocketAddr,
    native_tls: bool,
}

impl PrivateRelayServer {
    /// Starts the configured private relay with current membership admission.
    ///
    /// # Errors
    /// Returns [`PrivateRelayServerError`] when TLS loading or relay startup fails closed.
    pub async fn spawn(
        config: &PrivateRelayProviderConfig,
        access: PrivateRelayAccess,
    ) -> Result<Self, PrivateRelayServerError> {
        let (relay, native_tls) = match config.transport() {
            PrivateRelayTransport::NativeTls(tls) => {
                let server_config = tls
                    .load_server_config()
                    .map_err(PrivateRelayServerError::Tls)?;
                let mut relay = RelayConfig::new((config.listen_addr().ip(), 0));
                relay.tls = Some(TlsConfig::new(
                    config.listen_addr(),
                    CertConfig::Manual { server_config },
                ));
                (relay, true)
            }
            PrivateRelayTransport::ExternalTlsTermination => {
                (RelayConfig::new(config.listen_addr()), false)
            }
        };
        let mut relay = relay;
        relay.access = Arc::new(access.clone());
        let mut server_config = ServerConfig::default();
        server_config.relay = Some(relay);
        let server = Server::spawn(server_config)
            .await
            .map_err(PrivateRelayServerError::Spawn)?;
        let clients = server
            .relay_service()
            .ok_or(PrivateRelayServerError::MissingRelayService)?
            .clients()
            .clone();
        let listen_addr = if native_tls {
            server.https_addr()
        } else {
            server.http_addr()
        }
        .ok_or(PrivateRelayServerError::MissingListener)?;
        Ok(Self {
            server,
            access,
            clients,
            listen_addr,
            native_tls,
        })
    }

    /// Returns the actual relay listener address.
    pub const fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Returns whether the embedded relay terminates TLS itself.
    pub const fn uses_native_tls(&self) -> bool {
        self.native_tls
    }

    /// Applies coherent authorization state and disconnects sessions losing all eligibility.
    pub fn replace_from_spaces(&self, authorizations: &[SpaceAuthorizationView]) {
        for (endpoint_id, connection_id) in self.access.replace_from_spaces(authorizations) {
            self.clients.disconnect(endpoint_id, Some(connection_id));
        }
    }

    /// Gracefully stops the relay and all admitted connections.
    ///
    /// # Errors
    /// Returns [`PrivateRelayServerError`] when the relay supervisor fails.
    pub async fn shutdown(self) -> Result<(), PrivateRelayServerError> {
        self.server
            .shutdown()
            .await
            .map_err(PrivateRelayServerError::Supervisor)
    }
}

/// Private relay startup or lifecycle failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum PrivateRelayServerError {
    /// Native TLS material failed validation.
    Tls(RelayTlsError),
    /// Iroh relay server startup failed.
    Spawn(iroh_relay::server::SpawnError),
    /// Iroh relay supervisor failed during shutdown.
    Supervisor(iroh_relay::server::SupervisorError),
    /// Iroh started without exposing the requested listener.
    MissingListener,
    /// Iroh started without the configured relay service.
    MissingRelayService,
}

impl fmt::Display for PrivateRelayServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tls(error) => write!(formatter, "private relay TLS rejected: {error}"),
            Self::Spawn(error) => {
                write!(formatter, "private relay server failed to start: {error}")
            }
            Self::Supervisor(error) => {
                write!(formatter, "private relay supervisor failed: {error}")
            }
            Self::MissingListener => formatter.write_str("private relay listener was not created"),
            Self::MissingRelayService => {
                formatter.write_str("private relay service was not created")
            }
        }
    }
}

impl Error for PrivateRelayServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Tls(error) => Some(error),
            Self::Spawn(error) => Some(error),
            Self::Supervisor(error) => Some(error),
            Self::MissingListener | Self::MissingRelayService => None,
        }
    }
}
