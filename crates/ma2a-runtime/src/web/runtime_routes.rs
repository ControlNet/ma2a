use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};

use super::{WebState, cookie};

mod events;
mod snapshot_stream;
pub(super) use events::events;

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
    let Some(bearer) = cookie(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.auth.authenticate(bearer).await.is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
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
    let Some(bearer) = cookie(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.auth.authenticate(bearer).await.is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request = json!({"version": 1, "operation": "space_details_fetch", "space_id": id});
    let Ok(command) = crate::api::decode_command(request.to_string().as_bytes()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(runtime) = state.runtime.as_ref() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(response) = super::gateway::call(runtime, &command).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&response) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if value.get("error").and_then(Value::as_str) == Some("not_found") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if value.pointer("/result/type").and_then(Value::as_str) != Some("space_details") {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    value.pointer("/result/payload").map_or_else(
        || StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        |payload| Json(payload.clone()).into_response(),
    )
}

#[cfg(test)]
#[path = "runtime_routes_tests.rs"]
mod tests;
