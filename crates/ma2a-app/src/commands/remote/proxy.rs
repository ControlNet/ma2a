//! `ma2a proxy`: TCP destinations reached from the target, byte for byte.

use std::{sync::Arc, time::Duration};

use ma2a_core::service::{
    EndReason, MAX_PROXY_IDLE_MS, OpenBody, ProxyHost, ProxyRequest, ProxyResult,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client;
use tokio::net::TcpListener;

use super::{duration_ms, open, stdio::StdioGuard};
use crate::{
    AppError,
    cli::services::{ProxyCommand, ProxyDestination},
};

pub(crate) async fn run(paths: IpcPaths, command: ProxyCommand) -> Result<(), AppError> {
    match command {
        ProxyCommand::Connect { destination } => {
            let request = request(&destination)?;
            let idle = Duration::from_millis(request.idle_timeout_ms());
            let (mut reader, mut writer) =
                open(&paths, &destination.endpoint, OpenBody::Proxy(request)).await?;
            let stdio = StdioGuard::default();
            let local = (tokio::io::stdin(), stdio.stdout());
            let relayed = client::proxy::run(
                (&mut reader, &mut writer),
                local,
                (idle, super::process::interrupted()),
            )
            .await;
            exit_with(&stdio, relayed.map_err(AppError::from)).await
        }
        ProxyCommand::Forward {
            listen,
            destination,
        } => forward(paths, &listen, destination).await,
    }
}

/// Exits as `exec` does: 124 after a timeout, 130 after a cancel, and 255 after the target
/// Runtime shut down, so a script can tell a relay cut short from one that completed.
async fn exit_with(stdio: &StdioGuard, relayed: Result<ProxyResult, AppError>) -> ! {
    match relayed {
        Ok(result) => {
            let code = match result.reason {
                EndReason::TimedOut => {
                    stdio.note_stderr("proxy connection timed out").await;
                    124
                }
                EndReason::Cancelled => {
                    stdio.note_stderr("proxy connection cancelled").await;
                    130
                }
                EndReason::Shutdown => {
                    stdio.note_stderr("target Runtime shut down").await;
                    255
                }
                EndReason::Completed => 0,
            };
            stdio
                .exit_code(code, result.reason == EndReason::Completed)
                .await
        }
        Err(error) => stdio.exit_error(error).await,
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

/// Most connections forwarded at once: the size of the Runtime's local stream pool. Later ones
/// wait in the listen backlog.
const MAX_FORWARDED: usize = 64;
/// Pause after a failed accept before trying again.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

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
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_FORWARDED));
    loop {
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            return Ok(());
        };
        let (socket, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Running out of descriptors or an aborted handshake passes; the forwarder stays.
                eprintln!("accepting a connection failed: {error}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        let shared = Arc::clone(&shared);
        let request = request.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let (paths, endpoint) = shared.as_ref();
            let relayed = async {
                let idle = Duration::from_millis(request.idle_timeout_ms());
                let (mut reader, mut writer) =
                    open(paths, endpoint, OpenBody::Proxy(request)).await?;
                let (read, write) = socket.into_split();
                client::proxy::run(
                    (&mut reader, &mut writer),
                    (read, write),
                    (idle, std::future::pending()),
                )
                .await?;
                Ok::<(), AppError>(())
            };
            if let Err(error) = relayed.await {
                eprintln!("connection from {peer} failed: {error}");
            }
        });
    }
}
