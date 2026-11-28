//! Invitation creation and acceptance, the two commands that touch bearer material.

use std::{
    fmt::Write as _,
    io,
    io::IsTerminal as _,
    path::{Path, PathBuf},
};

use ma2a_core::RequestId;
use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Value, json};

use crate::AppError;

use super::{read_owner_only_invitation, space_reference::resolve_space};

/// The Space reference and lifetime one invitation is created for.
pub(super) struct InviteRequest<'a> {
    pub(super) space: &'a str,
    pub(super) ttl: &'a str,
}

/// Creates one invite and writes only the ticket to stdout.
///
/// The Runtime writes the bearer ticket to an owner-only file rather than
/// returning it in the local API response, so no invite secret ever enters a
/// JSON envelope, an argument vector, or a log.
pub(super) async fn invite(
    runtime: (&Path, IpcPaths),
    request: InviteRequest<'_>,
) -> Result<(), AppError> {
    let (state_dir, paths) = runtime;
    let space_id = resolve_space(state_dir, &paths, request.space).await?;
    let ttl_ms = parse_duration_ms(request.ttl)?;
    let output_path = invite_output_path(state_dir)?;
    let mut fields = crate::commands::workflows::request_fields()?;
    fields.insert("space_id".to_owned(), Value::String(space_id));
    fields.insert("ttl_ms".to_owned(), json!(ttl_ms));
    fields.insert(
        "output_path".to_owned(),
        Value::String(output_path.to_string_lossy().into_owned()),
    );
    let response = crate::request(
        (state_dir, paths),
        crate::commands::workflows::command("space_invite", fields)?,
    )
    .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let _cleanup = std::fs::remove_file(&output_path);
            return Err(error);
        }
    };
    let document: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
    if let Err(error) = crate::output::reject_runtime_error(&document) {
        let _cleanup = std::fs::remove_file(&output_path);
        return Err(error);
    }
    let invitation = read_owner_only_invitation(&output_path);
    let removed = std::fs::remove_file(&output_path);
    let invitation = invitation?;
    removed?;
    println!("{}", invitation.trim());
    Ok(())
}

/// Reads one invite ticket without ever accepting it from argv.
pub(super) async fn accept(
    state_dir: &Path,
    paths: IpcPaths,
    request_id: Option<String>,
) -> Result<(), AppError> {
    let request_id = match request_id {
        Some(value) => validate_request_id(value)?,
        None => request_id_hex(
            RequestId::random()
                .map_err(|_| io::Error::other("operating-system random source failed"))?,
        )?,
    };
    crate::daemon_control::require(&paths).await?;
    let invitation = read_invitation()?;
    let invitation = invitation.trim();
    if invitation.is_empty() {
        return Err(AppError::Usage("no invite ticket was supplied"));
    }
    let mut fields = serde_json::Map::new();
    fields.insert("request_id".to_owned(), Value::String(request_id.clone()));
    fields.insert(
        "invitation".to_owned(),
        Value::String(invitation.to_owned()),
    );
    let response = match crate::request(
        (state_dir, paths),
        crate::commands::workflows::command("space_redeem", fields)?,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return Err(attach_retry_request_id(error, &request_id, state_dir)),
    };
    let document: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
    if let Err(error) = crate::output::reject_runtime_error(&document) {
        return Err(if should_show_retry_request_id(&document) {
            attach_retry_request_id(error, &request_id, state_dir)
        } else {
            error
        });
    }
    crate::output::write_human(&document)?;
    Ok(())
}

fn should_show_retry_request_id(document: &Value) -> bool {
    let remediation = document.get("remediation").and_then(Value::as_str);
    if remediation.is_some_and(|text| text.contains("will not execute again")) {
        return false;
    }
    remediation.is_some_and(|text| text.contains("retry"))
        || document.get("error").and_then(Value::as_str) == Some("unavailable")
}

fn validate_request_id(value: String) -> Result<String, AppError> {
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(value)
    } else {
        Err(AppError::Usage(
            "--request-id must be 32 lowercase hex characters",
        ))
    }
}

fn attach_retry_request_id(error: AppError, request_id: &str, state_dir: &Path) -> AppError {
    let hint = retry_hint(state_dir, request_id);
    match error {
        AppError::Rejected(code, message) => AppError::Rejected(code, format!("{message}\n{hint}")),
        other => AppError::Protocol(format!("{other}\n{hint}")),
    }
}

fn retry_hint(state_dir: &Path, request_id: &str) -> String {
    quote_state_dir(state_dir).map_or_else(
        || {
            format!(
                "Keep the original --state-dir and retry the exact same ticket with: ma2a space accept --request-id {request_id}"
            )
        },
        |quoted| {
            format!(
                "Retry the exact same ticket with: ma2a --state-dir {quoted} space accept --request-id {request_id}"
            )
        },
    )
}

fn quote_state_dir(state_dir: &Path) -> Option<String> {
    let raw = state_dir.to_str()?;
    if raw.chars().any(char::is_control) {
        return None;
    }
    #[cfg(windows)]
    {
        if raw.contains('"') || raw.contains('%') {
            return None;
        }
        Some(format!("\"{raw}\""))
    }
    #[cfg(not(windows))]
    {
        Some(format!("'{}'", raw.replace('\'', "'\\''")))
    }
}

fn read_invitation() -> Result<String, AppError> {
    if io::stdin().is_terminal() {
        return prompt_invitation();
    }
    let mut input = String::new();
    io::Read::read_to_string(&mut io::stdin().lock(), &mut input)?;
    Ok(input)
}

/// Prompts on the terminal standard input is already attached to.
///
/// The ticket is bearer material, so the prompt reads it without echo. Reading
/// the same device that decided the prompt keeps the two consistent even when
/// the process has no controlling terminal of its own.
#[cfg(unix)]
fn prompt_invitation() -> Result<String, AppError> {
    const UNAVAILABLE: &str = "the terminal attached to standard input is unavailable";
    let terminal = rustix::termios::ttyname(io::stdin(), Vec::new())
        .map_err(|_| AppError::Usage(UNAVAILABLE))?
        .into_string()
        .map_err(|_| AppError::Usage(UNAVAILABLE))?;
    let config = rpassword::ConfigBuilder::new()
        .input_file_path(terminal.clone())
        .output_file_path(terminal)
        .build();
    rpassword::prompt_password_with_config("Invite: ", config).map_err(Into::into)
}

#[cfg(windows)]
fn prompt_invitation() -> Result<String, AppError> {
    rpassword::prompt_password("Invite: ").map_err(Into::into)
}

fn parse_duration_ms(value: &str) -> Result<u64, AppError> {
    let (amount, multiplier) = if let Some(amount) = value.strip_suffix("ms") {
        (amount, 1)
    } else if let Some(amount) = value.strip_suffix('s') {
        (amount, 1_000)
    } else if let Some(amount) = value.strip_suffix('m') {
        (amount, 60_000)
    } else {
        return Err(AppError::Usage("TTL must end in ms, s, or m"));
    };
    amount
        .parse::<u64>()
        .ok()
        .and_then(|amount| amount.checked_mul(multiplier))
        .filter(|ttl| (1..=300_000).contains(ttl))
        .ok_or(AppError::Usage("TTL must be between 1ms and 5m"))
}

fn invite_output_path(state_dir: &Path) -> Result<PathBuf, AppError> {
    let request_id = RequestId::random()
        .map_err(|_| io::Error::other("operating-system random source failed"))?;
    Ok(state_dir.join(format!(".invite-{}", request_id_hex(request_id)?)))
}

fn request_id_hex(request_id: RequestId) -> Result<String, AppError> {
    let mut encoded = String::with_capacity(32);
    for byte in request_id.as_bytes() {
        write!(&mut encoded, "{byte:02x}")
            .map_err(|_| io::Error::other("request identifier encoding failed"))?;
    }
    Ok(encoded)
}

#[cfg(test)]
#[path = "space_invite_tests.rs"]
mod tests;
