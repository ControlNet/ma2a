use std::io::{self, Write as _};

use serde_json::Value;

use crate::AppError;

pub(crate) fn write_json(response: &[u8]) -> io::Result<()> {
    io::stdout().lock().write_all(response)?;
    writeln!(io::stdout().lock())
}

/// Turns a Runtime error envelope into the typed protocol error it names.
///
/// Error envelopes carry `error` and `remediation` instead of a result, so every
/// command surfaces the real classification (`invalid_input`, `expired`,
/// `conflict`, `not_found`, …) rather than failing later on a missing result.
pub(crate) fn reject_runtime_error(document: &Value) -> Result<(), AppError> {
    let Some(error) = document.get("error").and_then(Value::as_str) else {
        return Ok(());
    };
    let message = document
        .get("remediation")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("Runtime rejected the request: {error}"),
            |remediation| format!("Runtime rejected the request: {error}\n{remediation}"),
        );
    Err(AppError::Protocol(message))
}

pub(crate) fn write_human(document: &Value) -> io::Result<()> {
    let result_type = document
        .pointer("/result/type")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("Runtime response is missing its result type"))?;
    let payload = document
        .pointer("/result/payload")
        .ok_or_else(|| io::Error::other("Runtime response is missing its result payload"))?;
    match result_type {
        "snapshot" => write_snapshot(payload),
        "endpoint_info" => write_endpoint(payload),
        "control_sync_status" | "control_sync_triggered" => write_control_sync(payload),
        "echo" => write_echo(payload),
        "spaces" => write_spaces(payload),
        "space"
        | "space_created"
        | "space_invitation_created"
        | "space_redeemed"
        | "space_revoked" => write_space(space_headline(result_type), payload),
        "space_left" => write_space_identity(payload),
        _ => writeln!(io::stdout().lock(), "{payload}"),
    }
}

fn space_headline(result_type: &str) -> Option<&'static str> {
    match result_type {
        "space_created" => Some("Created Space"),
        "space_invitation_created" => Some("Invited to Space"),
        "space_redeemed" => Some("Joined Space"),
        "space_revoked" => Some("Removed member from Space"),
        _ => None,
    }
}

fn write_space(headline: Option<&str>, payload: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    if let Some(headline) = headline {
        writeln!(stdout, "{headline}")?;
    }
    writeln!(stdout, "Name: {}", text(payload, "name"))?;
    writeln!(stdout, "Space ID: {}", text(payload, "space_id"))?;
    writeln!(
        stdout,
        "Members: {}",
        payload
            .get("member_count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    )
}

/// Departure reports only the Space it left; this Endpoint no longer holds
/// authoritative membership for it, so no member count is printed.
fn write_space_identity(payload: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "Left Space")?;
    writeln!(stdout, "Name: {}", text(payload, "name"))?;
    writeln!(stdout, "Space ID: {}", text(payload, "space_id"))
}

fn write_spaces(payload: &Value) -> io::Result<()> {
    let Some(spaces) = payload.as_array() else {
        return writeln!(io::stdout().lock(), "{payload}");
    };
    if spaces.is_empty() {
        return writeln!(io::stdout().lock(), "No Spaces");
    }
    let mut stdout = io::stdout().lock();
    for space in spaces {
        writeln!(
            stdout,
            "{}  {}  members={}",
            text(space, "name"),
            text(space, "space_id"),
            space
                .get("member_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        )?;
    }
    Ok(())
}

fn text<'a>(payload: &'a Value, field: &str) -> &'a str {
    payload
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("unavailable")
}

fn write_snapshot(payload: &Value) -> io::Result<()> {
    let endpoint = payload
        .pointer("/endpoint/endpoint_id")
        .and_then(Value::as_str)
        .unwrap_or("unavailable");
    let revision = payload
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    writeln!(io::stdout().lock(), "Endpoint: {endpoint}")?;
    writeln!(io::stdout().lock(), "Revision: {revision}")?;
    writeln!(
        io::stdout().lock(),
        "Configured relay candidates: {}",
        payload
            .get("relay_candidates")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    )?;
    writeln!(
        io::stdout().lock(),
        "Iroh-observed relay state: {}",
        payload
            .get("observed_relay_state")
            .map_or_else(|| "unavailable".to_owned(), Value::to_string)
    )?;
    writeln!(
        io::stdout().lock(),
        "Control sync: {}",
        payload
            .get("control_sync")
            .map_or_else(|| "unavailable".to_owned(), Value::to_string)
    )
}

fn write_endpoint(payload: &Value) -> io::Result<()> {
    writeln!(
        io::stdout().lock(),
        "Endpoint: {}",
        payload
            .get("endpoint_id")
            .and_then(Value::as_str)
            .unwrap_or("unavailable")
    )
}

fn write_control_sync(payload: &Value) -> io::Result<()> {
    writeln!(
        io::stdout().lock(),
        "Control sync: {}",
        payload
            .get("peer_endpoint_ids")
            .and_then(Value::as_array)
            .is_some_and(|peers| !peers.is_empty())
    )
}

fn write_echo(payload: &Value) -> io::Result<()> {
    writeln!(
        io::stdout().lock(),
        "{}",
        payload
            .get("payload")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::reject_runtime_error;
    use serde_json::json;

    #[test]
    fn runtime_error_envelopes_surface_their_real_protocol_error() {
        // Given
        let envelope = json!({"version": 1, "error": "expired", "remediation": null});

        // When
        let rejection = reject_runtime_error(&envelope).expect_err("expired must be rejected");

        // Then
        let message = rejection.to_string();
        assert!(message.contains("expired"), "{message}");
        assert!(!message.contains("missing its result type"), "{message}");
    }

    #[test]
    fn remediation_reaches_the_operator_and_success_envelopes_pass_through() {
        // Given
        let envelope = json!({
            "version": 1,
            "error": "unauthorized",
            "remediation": "Space owner cannot leave its own Space",
        });
        let success = json!({"version": 1, "result": {"type": "spaces", "payload": []}});

        // When
        let rejection = reject_runtime_error(&envelope).expect_err("unauthorized must be rejected");

        // Then
        assert!(
            rejection
                .to_string()
                .contains("Space owner cannot leave its own Space")
        );
        assert!(reject_runtime_error(&success).is_ok());
    }
}
