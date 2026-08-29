use std::{future::Future, net::SocketAddr};

use axum::Router;

use super::{WebAssets, WebAuthService, WebServerConfig, build_router};

/// Bound loopback Web server with an OS-selected port.
#[derive(Debug)]
pub struct LoopbackWebServer {
    listeners: Vec<tokio::net::TcpListener>,
    router: Router,
    port: u16,
}

impl LoopbackWebServer {
    /// Binds IPv4 and IPv6 loopback listeners on one ephemeral port.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when either loopback listener cannot be established.
    pub async fn bind(
        auth: WebAuthService,
        assets: WebAssets,
        config: WebServerConfig,
    ) -> std::io::Result<Self> {
        let ipv4 = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = ipv4.local_addr()?.port();
        let ipv6 = tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await?;
        Ok(Self {
            listeners: vec![ipv4, ipv6],
            router: build_router(auth, assets, config, port),
            port,
        })
    }

    /// Returns the selected loopback port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Serves both listeners until graceful shutdown completes.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when accepting or serving a loopback connection fails.
    pub async fn serve(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
        let shutdown_task = tokio::spawn(async move {
            shutdown.await;
            let _result = shutdown_sender.send(true);
        });
        let mut servers = tokio::task::JoinSet::new();
        for listener in self.listeners {
            let router = self.router.clone();
            let mut shutdown_receiver = shutdown_receiver.clone();
            servers.spawn(async move {
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    if !*shutdown_receiver.borrow() {
                        let _result = shutdown_receiver.changed().await;
                    }
                })
                .await
            });
        }
        drop(shutdown_receiver);
        while let Some(joined) = servers.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    servers.abort_all();
                    shutdown_task.abort();
                    return Err(error);
                }
                Err(error) => {
                    servers.abort_all();
                    shutdown_task.abort();
                    return Err(std::io::Error::other(error));
                }
            }
        }
        shutdown_task.abort();
        Ok(())
    }
}
