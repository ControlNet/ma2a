use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};

use super::{WebState, authenticated_read, cookie, read_status};

mod agents;
mod events;
mod hosted;
mod snapshot_stream;
pub(super) use agents::agents;
pub(super) use events::events;
pub(super) use hosted::hosted_services;

#[cfg(test)]
use events::{
    EVENT_CHANNEL_CAPACITY, EventCursor, HEARTBEAT_INTERVAL, POLL_INTERVAL, SnapshotChange,
    wait_for_poll,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnapshotStamp {
    revision: u64,
    boot_id: String,
}

pub(super) async fn snapshot(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Err(status) = authenticated_read(&state, &headers).await {
        return status.into_response();
    }
    match fetch_snapshot(&state).await {
        Ok((stamp, payload)) => snapshot_stream::response(stamp, payload),
        Err(status) => status.into_response(),
    }
}

async fn fetch_snapshot(state: &WebState) -> Result<(SnapshotStamp, Value), StatusCode> {
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let response = super::gateway::call(runtime, &crate::api::Command::snapshot_fetch())
        .await
        .map_err(|()| StatusCode::SERVICE_UNAVAILABLE)?;
    let value: Value =
        serde_json::from_slice(&response).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let boot_id = value
        .get("runtime_boot_id")
        .and_then(Value::as_str)
        .filter(|value| value.len() == 32)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .to_owned();
    let payload = value
        .pointer("/result/payload")
        .cloned()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((SnapshotStamp { revision, boot_id }, payload))
}

pub(super) async fn space_details(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let request = json!({"version": 1, "operation": "space_details_fetch", "space_id": id});
    read(&state, &headers, (&request, "space_details")).await
}

/// Every exposure this Runtime serves; consent itself is changed only through the CLI.
pub(super) async fn service_exposures(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response {
    let request = json!({"version": 1, "operation": "service_exposure_list"});
    read(&state, &headers, (&request, "service_exposures")).await
}

/// Active operations and the bounded recent audit, both content-free.
pub(super) async fn operations(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let request = json!({"version": 1, "operation": "operations_status"});
    read(&state, &headers, (&request, "operations")).await
}

/// The first page of hosted Processes, content-free: never their arguments or output.
pub(super) async fn processes(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let request = json!({"version": 1, "operation": "process_list", "after": null});
    read(&state, &headers, (&request, "processes")).await
}

/// Answers one authenticated read-only Local API command with its result payload.
async fn read(
    state: &WebState,
    headers: &HeaderMap,
    (request, result): (&Value, &str),
) -> Response {
    match payload(state, headers, (request, result)).await {
        Ok(payload) => Json(payload).into_response(),
        Err(status) => status.into_response(),
    }
}

/// The result payload of one authenticated read-only Local API command.
async fn payload(
    state: &WebState,
    headers: &HeaderMap,
    (request, result): (&Value, &str),
) -> Result<Value, StatusCode> {
    authenticated_read(state, headers).await?;
    let command = crate::api::decode_command(request.to_string().as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let response = super::gateway::call(runtime, &command)
        .await
        .map_err(|()| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut value = serde_json::from_slice::<Value>(&response)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if value.get("error").and_then(Value::as_str) == Some("not_found") {
        return Err(StatusCode::NOT_FOUND);
    }
    if value.pointer("/result/type").and_then(Value::as_str) != Some(result) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    value
        .pointer_mut("/result/payload")
        .map(Value::take)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
#[path = "runtime_routes_tests.rs"]
mod tests;
