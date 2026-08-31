use std::{io::Write as _, path::Path};

use ma2a_runtime::ipc::{IpcPaths, LocalApiClient};

use crate::AppError;

pub(crate) async fn run(state_dir: &Path) -> Result<(), AppError> {
    let paths = IpcPaths::new(state_dir)?;
    crate::autostart::ensure_daemon(state_dir, &paths).await?;
    let response = LocalApiClient::new(paths)
        .call(&crate::commands::workflows::unit_command("ui_open")?)
        .await?;
    let document: serde_json::Value =
        serde_json::from_slice(&response).map_err(std::io::Error::other)?;
    let url = document
        .pointer("/result/payload/url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| std::io::Error::other("Runtime response is missing the loopback UI URL"))?;
    writeln!(std::io::stdout().lock(), "{url}")?;
    Ok(())
}
