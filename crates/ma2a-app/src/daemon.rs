use std::{fs::TryLockError, path::PathBuf, sync::Arc};

use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{SystemClock, WebAuthConfig},
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
    let control = CurrentUserRuntime::open_at(
        &state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let web = ma2a_runtime::web::LoopbackWebServer::bind_with_runtime(
        ma2a_runtime::web::WebRuntimeDependencies::new(
            control.web_auth().clone(),
            ma2a_runtime::web::WebAssets::new(crate::embedded_web::WEB_ASSETS),
            LocalApiClient::new(paths.clone()),
        ),
        ma2a_runtime::web::WebServerConfig::default(),
    )
    .await
    .map_err(AppError::Web)?;
    let web_url = format!("http://127.0.0.1:{}", web.port());
    let serve_result = match LocalApiServer::bind(paths.clone(), runtime.handle(), control) {
        Ok(server) => {
            let cancellation = CancellationToken::new();
            let serving = server
                .with_web_url(web_url)
                .serve(cancellation.child_token());
            let web_serving = web.serve(cancellation.child_token().cancelled_owned());
            tokio::pin!(serving);
            tokio::pin!(web_serving);
            tokio::select! {
                result = &mut serving => result.map(|_exit| ()).map_err(AppError::from),
                result = &mut web_serving => result.map_err(AppError::Web),
                signal = tokio::signal::ctrl_c() => match signal {
                    Ok(()) => {
                        cancellation.cancel();
                        serving.await.map(|_exit| ()).map_err(AppError::from)?;
                        web_serving.await.map_err(AppError::Web)
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
