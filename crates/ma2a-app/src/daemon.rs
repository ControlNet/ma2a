use std::{fs::TryLockError, path::PathBuf, sync::Arc};

use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer, ServerExit},
    web::{SystemClock, WebAuthConfig},
};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;

use crate::{
    AppError,
    daemon_control::{READY_TOKEN, open_lock, wait_until_live},
};

/// Tells the parent that spawned this daemon that it can now serve commands.
///
/// A detached daemon has no other way to report this, and without it the parent
/// can only poll a socket against a guessed budget: it cannot tell a slow start
/// from a dead child, and a child that starts after the parent gave up is left
/// with no owner. One line on the inherited pipe answers both exactly. Only a
/// detached daemon writes it; a foreground one is already visible to its caller.
fn signal_ready(detached: bool) {
    use std::io::Write as _;

    if !detached {
        return;
    }
    let mut stdout = std::io::stdout();
    let _announced = writeln!(stdout, "{READY_TOKEN}");
    let _flushed = stdout.flush();
}

/// Reports a Runtime that stopped underneath the transport as a daemon failure.
///
/// Cancellation and a completed graceful shutdown are ordinary exits. A Runtime
/// that stopped on its own is not: the daemon can no longer answer anything, so
/// it must fail loudly and let its supervisor or the operator start a new one
/// rather than linger as an endpoint that accepts requests and abandons them.
fn finish_serving(result: Result<ServerExit, ma2a_runtime::ipc::IpcError>) -> Result<(), AppError> {
    match result {
        Ok(ServerExit::RuntimeStopped) => Err(std::io::Error::other(
            "the Runtime stopped; the daemon cannot answer any command",
        )
        .into()),
        Ok(_) => Ok(()),
        Err(error) => Err(AppError::from(error)),
    }
}

pub(crate) async fn run(
    state_dir: PathBuf,
    paths: IpcPaths,
    detach_session: bool,
) -> Result<(), AppError> {
    #[cfg(unix)]
    if detach_session {
        rustix::process::setsid().map_err(std::io::Error::from)?;
    }
    paths.prepare()?;
    let lock = open_lock(&paths.lock_path())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            wait_until_live(&LocalApiClient::new(paths)).await?;
            signal_ready(detach_session);
            return Ok(());
        }
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    if LocalApiClient::new(paths.clone()).is_live().await {
        signal_ready(detach_session);
        return Ok(());
    }
    paths.remove_stale_endpoint()?;
    let runtime = Runtime::start(StoreConfig::new(&state_dir)).await?;
    let control = CurrentUserRuntime::open_at(
        &state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let web = ma2a_runtime::web::WebLifecycle::new(
        control.web_auth().clone(),
        ma2a_runtime::web::WebRuntimeDependencies::new(
            control.web_auth().clone(),
            ma2a_runtime::web::WebAssets::new(crate::embedded_web::WEB_ASSETS),
            LocalApiClient::new(paths.clone()),
        ),
    );
    let serve_result = match LocalApiServer::bind(paths.clone(), runtime.handle(), control) {
        Ok(server) => {
            // The endpoint is bound, so a caller can connect from here on.
            signal_ready(detach_session);
            let cancellation = CancellationToken::new();
            let serving = server
                .with_web_lifecycle(web.clone())
                .serve(cancellation.child_token());
            tokio::pin!(serving);
            tokio::select! {
                result = &mut serving => finish_serving(result),
                signal = tokio::signal::ctrl_c() => {
                    cancellation.cancel();
                    match signal {
                        Ok(()) => finish_serving(serving.await),
                        Err(error) => Err(error.into()),
                    }
                }
            }
        }
        Err(error) => Err(error.into()),
    };
    web.stop().await;
    let shutdown_result = runtime.shutdown().await.map_err(AppError::from);
    let cleanup_result = paths.remove_stale_endpoint().map_err(AppError::from);
    drop(lock);
    serve_result?;
    shutdown_result?;
    cleanup_result
}
