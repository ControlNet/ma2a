use std::io::{self, Write as _};

use serde_json::Value;

pub(crate) fn write_json(response: &[u8]) -> io::Result<()> {
    io::stdout().lock().write_all(response)?;
    writeln!(io::stdout().lock())
}

pub(crate) fn write_human(response: &[u8]) -> io::Result<()> {
    let document: Value = serde_json::from_slice(response).map_err(io::Error::other)?;
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
        _ => writeln!(io::stdout().lock(), "{payload}"),
    }
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
            .get("synchronized")
            .and_then(Value::as_bool)
            .unwrap_or(false)
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
