//! Agent Cards: what one route's Agent is and how to reach it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use serde_json::json;

use super::{
    backend::Route,
    http::{self, Shared},
    rpc,
};

pub(super) async fn local(State(shared): State<Shared>, Path(agent): Path<String>) -> Response {
    match http::local(&agent) {
        Some(route) => card(&shared, &route).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn remote(
    State(shared): State<Shared>,
    Path((endpoint, agent)): Path<(String, String)>,
) -> Response {
    match http::remote(&endpoint, &agent) {
        Some(route) => card(&shared, &route).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The Agent Card of one route.
async fn card(shared: &Shared, route: &Route) -> Response {
    let Ok((description, _available)) = shared.backend.card(route).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = route.agent().as_str();
    let description = if description.is_empty() {
        format!("MA2A Agent {name}")
    } else {
        description
    };
    let card = json!({
        "name": name,
        "description": description,
        "supportedInterfaces": [{
            "url": format!("{}{}", shared.origin, route.path()),
            "protocolBinding": "JSONRPC",
            "protocolVersion": rpc::VERSION,
        }],
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": {"streaming": true, "pushNotifications": false, "extendedAgentCard": false},
        "securitySchemes": {"bearer": {"httpAuthSecurityScheme": {
            "scheme": "Bearer",
            "description": "the token in this adapter's token file",
        }}},
        "securityRequirements": [{"schemes": {"bearer": {"list": []}}}],
        "defaultInputModes": ["text/plain", "application/json"],
        "defaultOutputModes": ["text/plain", "application/json", "application/octet-stream"],
        "skills": [{"id": name, "name": name, "description": description, "tags": ["ma2a"]}],
    });
    http::json_response(StatusCode::OK, &card)
}
