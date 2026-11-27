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
    let status = decode_status(operation, &document)?;
    print_status(status, json_output)
}

fn decode_status<'a>(operation: &str, document: &'a Value) -> Result<&'a Value, AppError> {
    if document
        .get("remediation")
        .and_then(Value::as_str)
        .is_none()
    {
        match (operation, document.get("error").and_then(Value::as_str)) {
            ("ui_start", Some("conflict")) => {
                return Err(AppError::Rejected(
                    "conflict".to_owned(),
                    "Runtime rejected the request: conflict\nWebUI password is not set; run `ma2a ui init` first"
                        .to_owned(),
                ));
            }
            ("ui_start", Some("unavailable")) => {
                return Err(AppError::Rejected(
                    "unavailable".to_owned(),
                    "Runtime rejected the request: unavailable\nWebUI could not resolve or bind the requested address; check --host/--port and whether the port is in use"
                        .to_owned(),
                ));
            }
            _ => {}
        }
    }
    crate::output::reject_runtime_error(document)?;
    if document.pointer("/result/type").and_then(Value::as_str) != Some("ui_status") {
        return Err(AppError::Protocol(
            "Runtime returned an unexpected WebUI status".to_owned(),
        ));
    }
    document
        .pointer("/result/payload")
        .ok_or_else(|| AppError::Protocol("missing WebUI status".to_owned()))
}

fn parse_status(status: &Value) -> Result<(bool, Option<&str>), AppError> {
    let running = status
        .get("running")
        .and_then(Value::as_bool)
        .ok_or_else(|| AppError::Protocol("invalid WebUI status".to_owned()))?;
    if running {
        let url = status
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Protocol("missing WebUI URL".to_owned()))?;
        Ok((true, Some(url)))
    } else {
        Ok((false, None))
    }
}

fn print_status(status: &Value, json_output: bool) -> Result<(), AppError> {
    let (running, url) = parse_status(status)?;
    if json_output {
        println!("{status}");
    } else if let Some(url) = url.filter(|_| running) {
        println!("running\nURL: {url}");
    } else {
        println!("stopped\nURL: -");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AppError, decode_status, parse_status};

    #[test]
    fn runtime_errors_and_malformed_replies_exit_with_protocol_or_rejection_code() {
        for (operation, envelope, expected) in [
            (
                "ui_start",
                json!({"version": 1, "error": "conflict", "remediation": null}),
                "conflict",
            ),
            (
                "ui_start",
                json!({"version": 1, "error": "unavailable", "remediation": null}),
                "unavailable",
            ),
            (
                "ui_stop",
                json!({"version": 1, "error": "internal", "remediation": "restart daemon"}),
                "restart daemon",
            ),
            (
                "ui_status",
                json!({"version": 1, "error": "invalid_input", "remediation": null}),
                "invalid_input",
            ),
        ] {
            let err = decode_status(operation, &envelope).expect_err("error envelope must fail");
            assert_eq!(err.exit_code(), 1, "{err}");
            assert!(matches!(err, AppError::Rejected(..)), "{err:?}");
            assert!(err.to_string().contains(expected), "{err}");
        }

        for malformed in [
            json!({"version": 1, "result": {"type": "snapshot", "payload": {}}}),
            json!({"version": 1, "result": {"type": "ui_status"}}),
        ] {
            let err = decode_status("ui_status", &malformed).expect_err("malformed must fail");
            assert_eq!(err.exit_code(), 1, "{err}");
            assert!(matches!(err, AppError::Protocol(_)), "{err:?}");
        }

        for bad_payload in [
            json!({"running": "yes"}),
            json!({"running": true, "url": null}),
        ] {
            let err = parse_status(&bad_payload).expect_err("bad payload must fail");
            assert_eq!(err.exit_code(), 1, "{err}");
            assert!(matches!(err, AppError::Protocol(_)), "{err:?}");
        }
    }
}
