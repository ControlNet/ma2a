use serde_json::Value;

use super::{ApiError, Command, CommandResult, LOCAL_API_VERSION, UiAuthView};

/// Parses one successful UI-control response from the authenticated local daemon.
///
/// # Errors
/// Returns invalid input when the response is malformed or does not match the command.
pub fn decode_ui_control_response(
    command: &Command,
    input: &[u8],
) -> Result<CommandResult, ApiError> {
    let value: Value = serde_json::from_slice(input).map_err(|_| ApiError::invalid_input())?;
    if value.get("version").and_then(Value::as_u64) != Some(u64::from(LOCAL_API_VERSION)) {
        return Err(ApiError::invalid_input());
    }
    let result = value.get("result").ok_or_else(ApiError::invalid_input)?;
    let payload = result.get("payload").ok_or_else(ApiError::invalid_input)?;
    let initialized = payload
        .get("initialized")
        .and_then(Value::as_bool)
        .ok_or_else(ApiError::invalid_input)?;
    let password_set = payload
        .get("password_set")
        .and_then(Value::as_bool)
        .ok_or_else(ApiError::invalid_input)?;
    let active_sessions = payload
        .get("active_sessions")
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(ApiError::invalid_input)?;
    let auth = UiAuthView::new(initialized, password_set, active_sessions);
    match (
        command.operation(),
        result.get("type").and_then(Value::as_str),
    ) {
        ("ui_password_set", Some("ui_password_set")) => Ok(CommandResult::ui_password_set(auth)),
        ("ui_password_reset", Some("ui_password_reset")) => {
            Ok(CommandResult::ui_password_reset(auth))
        }
        ("session_revoke_all", Some("sessions_revoked")) => {
            Ok(CommandResult::sessions_revoked(auth))
        }
        _ => Err(ApiError::invalid_input()),
    }
}
