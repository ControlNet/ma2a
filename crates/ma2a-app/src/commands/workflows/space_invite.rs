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
    let invitation = read_owner_only_invitation(&output_path)?;
    std::fs::remove_file(&output_path)?;
    println!("{}", invitation.trim());
    Ok(())
}

/// Reads one invite ticket without ever accepting it from argv.
pub(super) async fn accept(state_dir: &Path, paths: IpcPaths) -> Result<(), AppError> {
    crate::daemon_control::require(&paths).await?;
    let invitation = read_invitation()?;
    let invitation = invitation.trim();
    if invitation.is_empty() {
        return Err(AppError::Usage("no invite ticket was supplied"));
    }
    let mut fields = crate::commands::workflows::request_fields()?;
    fields.insert(
        "invitation".to_owned(),
        Value::String(invitation.to_owned()),
    );
    crate::call(
        (state_dir, paths),
        crate::commands::workflows::command("space_redeem", fields)?,
        false,
    )
    .await
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
mod tests {
    use super::parse_duration_ms;

    #[test]
    fn the_default_invite_lifetime_is_five_minutes() {
        // Given / When / Then
        assert_eq!(
            parse_duration_ms(crate::cli::DEFAULT_INVITE_TTL).ok(),
            Some(300_000)
        );
    }

    #[test]
    fn invite_lifetimes_stay_inside_the_protocol_bound() {
        // Given / When / Then
        assert_eq!(parse_duration_ms("30s").ok(), Some(30_000));
        assert_eq!(parse_duration_ms("1ms").ok(), Some(1));
        assert!(parse_duration_ms("6m").is_err());
        assert!(parse_duration_ms("0s").is_err());
        assert!(parse_duration_ms("5").is_err());
    }
}
