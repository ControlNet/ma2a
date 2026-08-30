use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde_json::Value;

use super::WebState;

pub(super) async fn mutation(State(state): State<WebState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !super::headers::valid_same_origin(&headers, state.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match super::authenticated_mutation(&state, &headers).await {
        Ok(_) => {}
        Err(response) => return response,
    }
    if super::headers::single_header_value(&headers, &header::CONTENT_TYPE)
        != Some("application/json")
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Some(operation) = mutation_operation(parts.uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(body) = to_bytes(body, crate::api::MAX_LOCAL_REQUEST_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(command) = crate::api::decode_command(&body) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    if command.operation() != operation {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let Some(runtime) = state.runtime.as_ref() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    runtime.call(&command).await.map_or_else(
        |_| StatusCode::SERVICE_UNAVAILABLE.into_response(),
        |response| {
            serde_json::from_slice::<Value>(&response).map_or_else(
                |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                |payload| Json(payload).into_response(),
            )
        },
    )
}

fn mutation_operation(path: &str) -> Option<&'static str> {
    match path {
        "/api/v1/spaces/create" => Some("space_create"),
        "/api/v1/spaces/invite" => Some("space_invite"),
        "/api/v1/spaces/redeem" => Some("space_redeem"),
        "/api/v1/spaces/revoke" => Some("space_revoke"),
        "/api/v1/control-sync/trigger" => Some("control_sync_trigger"),
        "/api/v1/relays/private/configure" => Some("private_relay_configure"),
        "/api/v1/relays/public/configure" => Some("public_relay_configure"),
        "/api/v1/echo" => Some("echo_call"),
        "/api/v1/sessions/revoke-all" => Some("session_revoke_all"),
        _ => None,
    }
}
