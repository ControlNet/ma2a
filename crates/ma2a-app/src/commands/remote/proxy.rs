//! `ma2a proxy`: TCP destinations reached from the target, byte for byte.

use std::sync::Arc;

use ma2a_core::service::{MAX_PROXY_IDLE_MS, OpenBody, ProxyHost, ProxyRequest};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client;
use tokio::net::TcpListener;

use super::{duration_ms, open};
use crate::{
    AppError,
    cli::services::{ProxyCommand, ProxyDestination},
};

pub(crate) async fn run(paths: IpcPaths, command: ProxyCommand) -> Result<(), AppError> {
    match command {
        ProxyCommand::Connect { destination } => {
            let request = request(&destination)?;
            let (mut reader, mut writer) =
                open(&paths, &destination.endpoint, OpenBody::Proxy(request)).await?;
            let local = (tokio::io::stdin(), tokio::io::stdout());
            let relayed = client::proxy::run((&mut reader, &mut writer), local).await;
            super::exit_with(relayed.map(drop).map_err(AppError::from))
        }
        ProxyCommand::Forward {
            listen,
            destination,
        } => forward(paths, &listen, destination).await,
    }
}

fn request(destination: &ProxyDestination) -> Result<ProxyRequest, AppError> {
    let host = ProxyHost::new(&destination.host).map_err(|_| {
        AppError::Usage("HOST must be a lowercase DNS name or a canonical IP literal")
    })?;
    let idle = duration_ms(&destination.idle, MAX_PROXY_IDLE_MS)?;
    ProxyRequest::new(host, destination.port, idle)
        .map_err(|_| AppError::Usage("PORT must be between 1 and 65535"))
}

/// Relays every accepted local connection to the destination on its own stream.
async fn forward(
    paths: IpcPaths,
    listen: &str,
    destination: ProxyDestination,
) -> Result<(), AppError> {
    let request = request(&destination)?;
    crate::daemon_control::require(&paths).await?;
    let listener = TcpListener::bind(listen).await?;
    eprintln!(
        "forwarding {} to {}:{} through the target Endpoint; press Ctrl-C to stop",
        listener.local_addr()?,
        destination.host,
        destination.port
    );
    let shared = Arc::new((paths, destination.endpoint));
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let request = request.clone();
        tokio::spawn(async move {
            let (paths, endpoint) = shared.as_ref();
            let relayed = async {
                let (mut reader, mut writer) =
                    open(paths, endpoint, OpenBody::Proxy(request)).await?;
                let (read, write) = socket.into_split();
                client::proxy::run((&mut reader, &mut writer), (read, write)).await?;
                Ok::<(), AppError>(())
            };
            if let Err(error) = relayed.await {
                eprintln!("connection from {peer} failed: {error}");
            }
        });
    }
}
