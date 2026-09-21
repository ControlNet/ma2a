use std::{net::SocketAddr, sync::Arc};

use ma2a_core::ProtocolError;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{WebAuthService, WebRuntimeDependencies, WebServerConfig};
use crate::api::UiStatusView;

/// Daemon-owned Web UI service. New instances are always stopped.
#[derive(Clone, Debug)]
pub struct WebLifecycle {
    auth: WebAuthService,
    dependencies: WebRuntimeDependencies,
    running: Arc<Mutex<Option<RunningWeb>>>,
}

#[derive(Debug)]
struct RunningWeb {
    address: SocketAddr,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Drop for RunningWeb {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

impl WebLifecycle {
    /// Creates an explicitly controlled UI service without opening a listener.
    pub fn new(auth: WebAuthService, dependencies: WebRuntimeDependencies) -> Self {
        Self {
            auth,
            dependencies,
            running: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts or restarts the UI, returning only after the listener is bound.
    ///
    /// # Errors
    /// Returns conflict before password initialization or unavailable when the
    /// requested host cannot be resolved or its socket cannot be bound.
    pub async fn start(&self, host: &str, port: u16) -> Result<UiStatusView, ProtocolError> {
        let mut running = self.running.lock().await;
        if self
            .auth
            .setup_required()
            .await
            .map_err(|_| ProtocolError::INTERNAL)?
        {
            return Err(ProtocolError::CONFLICT);
        }
        let addresses: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?
        .map_err(|_| ProtocolError::UNAVAILABLE)?
        .collect();
        // Reusing the same socket requires releasing the old listener first.
        // A different requested socket can be bound before stopping the old service.
        if running.as_ref().is_some_and(|web| {
            addresses.iter().any(|address| {
                address.port() == web.address.port()
                    && (address.ip() == web.address.ip()
                        || address.ip().is_unspecified()
                        || web.address.ip().is_unspecified())
            })
        }) {
            stop_running(&mut running).await;
        }
        let server = ManagedWebServer::bind_at(self.dependencies.clone(), host, &addresses)
            .await
            .map_err(|_| ProtocolError::UNAVAILABLE)?;
        let address = server
            .listener
            .local_addr()
            .map_err(|_| ProtocolError::INTERNAL)?;
        stop_running(&mut running).await;
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(server.serve(cancellation.clone()));
        *running = Some(RunningWeb {
            address,
            cancellation,
            task,
        });
        drop(running);
        Ok(UiStatusView::new(Some(address)))
    }

    /// Stops HTTP connections and listeners without stopping the Endpoint.
    pub async fn stop(&self) -> UiStatusView {
        let mut running = self.running.lock().await;
        stop_running(&mut running).await;
        UiStatusView::new(None)
    }

    /// Reports the live task and its actual listener URL.
    pub async fn status(&self) -> UiStatusView {
        let running = self.running.lock().await;
        UiStatusView::new(
            running
                .as_ref()
                .filter(|web| !web.task.is_finished())
                .map(|web| web.address),
        )
    }
}

async fn stop_running(running: &mut Option<RunningWeb>) {
    if let Some(mut web) = running.take() {
        web.cancellation.cancel();
        let _result = (&mut web.task).await;
    }
}

struct ManagedWebServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
}

impl ManagedWebServer {
    async fn bind_at(
        dependencies: WebRuntimeDependencies,
        host: &str,
        addresses: &[SocketAddr],
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addresses).await?;
        let address = listener.local_addr()?;
        let config = WebServerConfig {
            binding: Some(Arc::new(super::router::WebBinding {
                host: host.to_owned(),
                address,
            })),
            ..WebServerConfig::default()
        };
        Ok(Self {
            listener,
            router: super::build_runtime_router(dependencies, config, address.port()),
        })
    }

    // Own every connection task so stop also closes SSE and incomplete requests.
    async fn serve(self, cancellation: CancellationToken) -> std::io::Result<()> {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break,
                accepted = self.listener.accept() => {
                    let (stream, peer) = accepted?;
                    let router = self.router.clone().layer(axum::Extension(axum::extract::ConnectInfo(peer)));
                    connections.spawn(async move {
                        let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
                        let _result = builder.serve_connection(
                            hyper_util::rt::TokioIo::new(stream),
                            hyper_util::service::TowerToHyperService::new(router),
                        ).await;
                    });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        drop(self.listener);
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}
