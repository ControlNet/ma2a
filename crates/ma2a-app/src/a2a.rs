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
mod rpc;
mod stream;
mod time;
mod view;

/// How long a stop waits for open requests and streams to finish.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Writes `token` to `path`, readable only by this user, replacing any earlier file.
///
/// The staging name is this process's own, so two adapters starting together never remove
/// or install each other's staged token.
fn write_private(path: &Path, token: &str) -> io::Result<()> {
    let staging = path.with_extension(format!("ma2a-partial-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let _stale = std::fs::remove_file(&staging);
    let mut file = options.open(&staging)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&staging, path)
}

/// Claims the token file at `path` for as long as the returned lock file stays open.
///
/// A second adapter naming the same token file is refused here, before it writes anything,
/// so a running adapter's published token file is never replaced underneath its clients.
/// The lock file itself stays behind empty; removing it could let two claims race.
fn claim(path: &Path) -> Result<std::fs::File, AppError> {
    let mut name = path
        .file_name()
        .ok_or(AppError::Usage("--token-file must name a file"))?
        .to_os_string();
    name.push(".lock");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let lock = options.open(path.with_file_name(name))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => Err(AppError::Usage(
            "another adapter is serving with this token file; give this one its own --token-file",
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Removes `path` only while it still holds `token`: another adapter that wrote its own token
/// there since keeps it when this one stops.
fn remove_if_ours(path: &Path, token: &str) {
    if std::fs::read(path).is_ok_and(|held| held == token.as_bytes()) {
        let _removed = std::fs::remove_file(path);
    }
}

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
    let _claim = claim(&token_file)?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| io::Error::other("random source failed"))?;
    let token = blake3::Hash::from_bytes(secret).to_hex().to_string();
    write_private(&token_file, &token)?;
    let ready = json!({"url": origin, "token_file": token_file.display().to_string()});
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
    let serving = axum::serve(listener, http::router(shared)).with_graceful_shutdown(async move {
        stopped().await;
        signalled.cancel();
    });
    let outcome = tokio::select! {
        finished = serving => finished,
        () = async {
            stopping.cancelled().await;
            tokio::time::sleep(STOP_GRACE).await;
        } => Ok(()),
    };
    remove_if_ours(&token_file, &token);
    Ok(outcome?)
}

#[cfg(test)]
mod tests {
    use super::{claim, remove_if_ours, write_private};

    #[test]
    fn a_second_adapter_on_a_claimed_token_file_is_refused_until_the_first_stops()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!("ma2a-a2a-claim-{}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("a2a-token");

        let first = claim(&path).map_err(|error| error.to_string())?;
        let refused = claim(&path).err().map(|error| error.to_string());
        assert!(refused.is_some_and(|message| message.contains("--token-file")));
        assert!(claim(&directory.join("other-token")).is_ok());
        drop(first);
        assert!(claim(&path).is_ok());

        std::fs::remove_dir_all(&directory)?;
        Ok(())
    }

    #[test]
    fn a_stopping_adapter_leaves_a_token_another_adapter_wrote_since()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!("ma2a-a2a-token-{}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("a2a-token");
        write_private(&path, "first")?;
        write_private(&path, "second")?;

        remove_if_ours(&path, "first");
        assert_eq!(std::fs::read_to_string(&path)?, "second");
        remove_if_ours(&path, "second");
        assert!(!path.exists());

        std::fs::remove_dir_all(&directory)?;
        Ok(())
    }
}
