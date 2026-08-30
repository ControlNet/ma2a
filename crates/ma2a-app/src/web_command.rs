use std::{io::Write as _, path::Path, sync::Arc};

use ma2a_runtime::{
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient},
    web::{
        LoopbackWebServer, SystemClock, WebAssets, WebAuthConfig, WebRuntimeDependencies,
        WebServerConfig,
    },
};

use crate::{AppError, embedded_web};

pub(crate) async fn run(state_dir: &Path) -> Result<(), AppError> {
    let runtime = CurrentUserRuntime::open_at(
        state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let server = LoopbackWebServer::bind_with_runtime(
        WebRuntimeDependencies::new(
            runtime.web_auth().clone(),
            WebAssets::new(embedded_web::WEB_ASSETS),
            LocalApiClient::new(IpcPaths::new(state_dir).map_err(AppError::Ipc)?),
        ),
        WebServerConfig::default(),
    )
    .await
    .map_err(AppError::Web)?;
    writeln!(
        std::io::stdout().lock(),
        "MA2A Web: http://127.0.0.1:{}",
        server.port()
    )
    .map_err(AppError::Io)?;
    server
        .serve(async {
            let _result = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(AppError::Web)
}
