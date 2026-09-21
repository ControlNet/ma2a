use std::{io, path::Path};

use ma2a_runtime::ipc::{IpcPaths, LocalApiClient};
use serde_json::{Map, Value, json};

use crate::{AppError, cli::UiCommand};

pub(super) async fn run(state_dir: &Path, action: UiCommand) -> Result<(), AppError> {
    let (operation, fields, json_output) = match action {
        UiCommand::Init => return crate::credential_command::run_password(state_dir).await,
        UiCommand::RevokeAll => return crate::credential_command::run_revoke_all(state_dir).await,
        UiCommand::Start { host, port, json } => (
            "ui_start",
            Map::from_iter([
                ("host".to_owned(), json!(host)),
                ("port".to_owned(), json!(port)),
            ]),
            json,
        ),
        UiCommand::Stop => ("ui_stop", Map::new(), false),
        UiCommand::Status { json } => ("ui_status", Map::new(), json),
    };
    let paths = IpcPaths::new(state_dir)?;
    let client = LocalApiClient::new(paths.clone());
    crate::daemon_control::require(&paths).await?;
    let command = super::workflows::command(operation, fields)?;
    let response = client.call(&command).await?;
    let document: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
    if let Some(error) = document.get("error").and_then(Value::as_str) {
        let message = match (operation, error) {
            ("ui_start", "conflict") => "WebUI password is not set; run `ma2a ui init` first",
            ("ui_start", "unavailable") => {
                "WebUI could not resolve or bind the requested address; check --host/--port and whether the port is in use"
            }
            _ => "WebUI control request failed; check daemon status",
        };
        return Err(AppError::Usage(message));
    }
    if document.pointer("/result/type").and_then(Value::as_str) != Some("ui_status") {
        return Err(AppError::Usage(
            "Runtime returned an unexpected WebUI status",
        ));
    }
    print_status(
        document
            .pointer("/result/payload")
            .ok_or(AppError::Usage("missing WebUI status"))?,
        json_output,
    )
}

fn print_status(status: &Value, json_output: bool) -> Result<(), AppError> {
    let running = status
        .get("running")
        .and_then(Value::as_bool)
        .ok_or(AppError::Usage("invalid WebUI status"))?;
    if json_output {
        println!("{status}");
    } else if running {
        let url = status
            .get("url")
            .and_then(Value::as_str)
            .ok_or(AppError::Usage("missing WebUI URL"))?;
        println!("running\nURL: {url}");
    } else {
        println!("stopped\nURL: -");
    }
    Ok(())
}
