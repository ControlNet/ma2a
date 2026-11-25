//! Agent Cards: what one route's Agent is and how to reach it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use serde_json::json;

use super::{
    backend::{Refusal, Route},
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

impl Refusal {
    /// Whether the Runtime or target could not answer now and may answer later.
    pub(super) fn is_transient(&self) -> bool {
        [
            "unavailable",
            "concurrency_exceeded",
            "unreachable",
            "timed_out",
            "outcome_unknown",
            "cancelled",
        ]
        .contains(&self.0.as_str())
    }
}

/// The Agent Card of one route.
async fn card(shared: &Shared, route: &Route) -> Response {
    let (description, _available) = match shared.backend.card(route).await {
        Ok(card) => card,
        // A Runtime or target that cannot answer now may answer later; any other refusal
        // names no Agent this caller can reach.
        Err(refusal) if refusal.is_transient() => {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
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

#[cfg(test)]
mod tests {
    #[test]
    fn an_endpoint_segment_that_is_no_endpoint_id_names_no_route() {
        // 64 lowercase hex characters, but not an Ed25519 public key: no Runtime could be
        // asked for it, so the route does not exist rather than being unavailable.
        let not_a_key = format!("02{}", "00".repeat(31));
        let mut bytes = [0_u8; 32];
        bytes[0] = 2;
        assert!(ma2a_core::EndpointId::try_from(bytes.as_slice()).is_err());
        assert!(super::super::http::remote(&not_a_key, "worker").is_none());
        let key = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
        assert!(super::super::http::remote(key, "worker").is_some());
    }

    /// The status of a remote card request through a target answering every stream with `script`.
    #[cfg(unix)]
    async fn served(script: crate::test_daemon::Script) -> crate::test_daemon::TestResult<u16> {
        let daemon = crate::test_daemon::Daemon::start("card", script)?;
        let (backend, route) = super::super::backend::Backend::standing_in(&daemon)?;
        let shared = super::Shared {
            backend,
            token: "token".into(),
            origin: "http://127.0.0.1:1".into(),
        };
        Ok(super::card(&shared, &route).await.status().as_u16())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_an_agent_the_target_does_not_list_answers_not_found()
    -> crate::test_daemon::TestResult {
        use ma2a_core::service::{FrameKind, ServiceError};
        let refused = |error: ServiceError| vec![(FrameKind::Reject, vec![error.code()])];
        assert_eq!(served(refused(ServiceError::Unauthorized)).await?, 404);
        // A target that cannot answer now may answer later.
        assert_eq!(served(refused(ServiceError::Unavailable)).await?, 503);
        assert_eq!(served(refused(ServiceError::Unreachable)).await?, 503);
        Ok(())
    }
}
