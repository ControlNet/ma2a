use std::{fs::TryLockError, path::PathBuf};

use ma2a_runtime::{
    Runtime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;

use crate::{
    AppError,
    autostart::{open_lock, wait_until_live},
};

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
        Err(TryLockError::WouldBlock) => return wait_until_live(&LocalApiClient::new(paths)).await,
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    if LocalApiClient::new(paths.clone()).is_live().await {
        return Ok(());
    }
    paths.remove_stale_endpoint()?;
    let runtime = Runtime::start(StoreConfig::new(&state_dir)).await?;
    let serve_result = match LocalApiServer::bind(paths.clone(), runtime.handle()) {
        Ok(server) => {
            let cancellation = CancellationToken::new();
            let serving = server.serve(cancellation.child_token());
            tokio::pin!(serving);
            tokio::select! {
                result = &mut serving => result.map(|_exit| ()).map_err(AppError::from),
                signal = tokio::signal::ctrl_c() => match signal {
                    Ok(()) => {
                        cancellation.cancel();
                        serving.await.map(|_exit| ()).map_err(AppError::from)
                    }
                    Err(error) => Err(error.into()),
                }
            }
        }
        Err(error) => Err(error.into()),
    };
    let shutdown_result = runtime.shutdown().await.map_err(AppError::from);
    let cleanup_result = paths.remove_stale_endpoint().map_err(AppError::from);
    drop(lock);
    serve_result?;
    shutdown_result?;
    cleanup_result
}
