//! `ma2a a2a serve`: this Runtime's Agents, and other Endpoints' through it, as an A2A v1.0
//! JSON-RPC 2.0 server over HTTP (`docs/protocol/a2a-adapter.md`).
//!
//! The adapter is a same-user local API client. A2A callers have no Endpoint identity, so
//! it acts as this Runtime's local operator for its own Agents and as this Endpoint for
//! another Endpoint's. It listens on loopback only, behind a bearer token that is random
//! per run, written to a file only this user can read, and never printed or logged.

use std::{
    io::{self, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use ma2a_runtime::ipc::IpcPaths;
use serde_json::json;

use crate::AppError;

mod artifact;
mod backend;
mod card;
mod http;
mod list;
mod mapping;
mod methods;
mod remote;
#[cfg(all(test, unix))]
mod remote_fixture;
mod rpc;
mod stream;
mod time;
mod token_file;
mod view;

/// How long a stop waits for open requests and streams to finish.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolves once the adapter is asked to stop: Ctrl-C, or `SIGTERM` on Unix.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _interrupted = tokio::signal::ctrl_c() => {}
                _terminated = terminate.recv() => {}
            }
            return;
        }
    }
    let _interrupted = tokio::signal::ctrl_c().await;
}

/// Serves until stopped, then removes the token file.
pub(crate) async fn serve(
    state_dir: &Path,
    paths: IpcPaths,
    (listen, token_file): (SocketAddr, Option<PathBuf>),
) -> Result<(), AppError> {
    if !listen.ip().is_loopback() {
        return Err(AppError::Usage(
            "--listen takes a loopback address: the adapter acts as this Runtime's operator",
        ));
    }
    crate::daemon_control::require(&paths).await?;
    let token_file = token_file.unwrap_or_else(|| state_dir.join("a2a-token"));
    let claim = token_file::claim(&token_file)?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| io::Error::other("random source failed"))?;
    let token = blake3::Hash::from_bytes(secret).to_hex().to_string();
    token_file::write_private(claim.path(), &token)?;
    let _published = token_file::Published {
        path: claim.path(),
        token: &token,
    };
    let ready = json!({"url": origin, "token_file": claim.path().display().to_string()});
    writeln!(io::stdout().lock(), "{ready}")?;
    let shared = http::Shared {
        backend: backend::Backend {
            state_dir: state_dir.to_path_buf(),
            paths,
        },
        token: Arc::from(token.as_str()),
        origin: Arc::from(origin),
    };
    // Open streams may last as long as their tasks, so a stop waits for them only briefly.
    let stopping = tokio_util::sync::CancellationToken::new();
    let signalled = stopping.clone();
    let signal = tokio::spawn(async move {
        stopped().await;
        signalled.cancel();
    });
    ma2a_runtime::web::serve_connections(listener, http::router(shared), (stopping, STOP_GRACE))
        .await;
    signal.abort();
    Ok(())
}
