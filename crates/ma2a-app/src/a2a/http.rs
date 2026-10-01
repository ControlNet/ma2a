//! HTTP routes: JSON-RPC, Agent Cards, and artifact content, all behind one bearer token.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use ma2a_core::service::AgentId;
use serde_json::Value;

use super::{
    artifact,
    backend::{Backend, Route},
    card, methods, rpc,
    stream::{self, Lifecycle, Owned},
    view::Served,
};

/// Largest JSON-RPC request body; one message is at most 12 KiB of content.
const MAX_REQUEST: usize = 65_536;

/// What every route shares.
#[derive(Clone, Debug)]
pub(super) struct Shared {
    pub(super) backend: Backend,
    /// The bearer token every request presents.
    pub(super) token: Arc<str>,
    /// `http://HOST:PORT` this adapter listens on.
    pub(super) origin: Arc<str>,
}

pub(super) fn router(shared: Shared) -> Router {
    let routes = Router::new()
        .route("/agents/{agent}", post(local_rpc))
        .route(
            "/agents/{agent}/.well-known/agent-card.json",
            get(card::local),
        )
        .route(
            "/agents/{agent}/tasks/{task}/artifacts/{name}",
            get(local_artifact),
        )
        .route("/endpoints/{endpoint}/agents/{agent}", post(remote_rpc))
        .route(
            "/endpoints/{endpoint}/agents/{agent}/.well-known/agent-card.json",
            get(card::remote),
        )
        .route(
            "/endpoints/{endpoint}/agents/{agent}/tasks/{task}/artifacts/{name}",
            get(remote_artifact),
        );
    routes
        .layer(middleware::from_fn_with_state(shared.clone(), authenticate))
        .layer(DefaultBodyLimit::max(MAX_REQUEST))
        .with_state(shared)
}

/// Compares two byte strings in time independent of where they differ.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

async fn authenticate(State(shared): State<Shared>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if presented.is_some_and(|token| same(token.as_bytes(), shared.token.as_bytes())) {
        return next.run(request).await;
    }
    let mut refused = StatusCode::UNAUTHORIZED.into_response();
    refused
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    refused
}

pub(super) fn local(agent: &str) -> Option<Route> {
    AgentId::new(agent).ok().map(Route::Local)
}

pub(super) fn remote(endpoint: &str, agent: &str) -> Option<Route> {
    let endpoint_ok = endpoint.len() == 64
        && endpoint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    endpoint_ok
        .then(|| AgentId::new(agent).ok())
        .flatten()
        .map(|agent| Route::Remote(endpoint.to_owned(), agent))
}

pub(super) fn json_response(status: StatusCode, value: &Value) -> Response {
    let mut response = (status, value.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

async fn local_rpc(
    State(shared): State<Shared>,
    Path(agent): Path<String>,
    request: Request,
) -> Response {
    match local(&agent) {
        Some(route) => dispatch(shared, route, request).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn remote_rpc(
    State(shared): State<Shared>,
    Path((endpoint, agent)): Path<(String, String)>,
    request: Request,
) -> Response {
    match remote(&endpoint, &agent) {
        Some(route) => dispatch(shared, route, request).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Answers one JSON-RPC request on `route`: a stream method as events, any other as one
/// JSON response.
async fn dispatch(shared: Shared, route: Route, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_REQUEST).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let request = match rpc::parse(&body) {
        Ok(request) => request,
        Err((id, error)) => return json_response(StatusCode::OK, &error.response(&id)),
    };
    if let Err(error) = rpc::version(&parts.headers, parts.uri.query()) {
        return json_response(StatusCode::OK, &error.response(&request.id));
    }
    let base = format!("{}{}", shared.origin, route.path());
    let served = Served {
        backend: &shared.backend,
        route: &route,
        base: &base,
    };
    let streamed = match request.method.as_str() {
        "SendStreamingMessage" => Some(start_streaming(served, &request.params).await),
        "SubscribeToTask" => Some(start_subscription(served, &request.params).await),
        _ => None,
    };
    let answer = match streamed {
        Some(Ok((method, task, until_waiting))) => {
            let owned = Owned {
                backend: shared.backend.clone(),
                route: route.clone(),
                base,
            };
            let lifecycle = Lifecycle {
                id: request.id,
                method,
                task,
                until_waiting,
            };
            return stream::respond(owned, lifecycle);
        }
        Some(Err(error)) => Err(error),
        None => methods::call(served, &request).await,
    };
    let response = match answer {
        Ok(result) => rpc::success(&request.id, &result),
        Err(error) => error.response(&request.id),
    };
    json_response(StatusCode::OK, &response)
}

type Started = Result<(&'static str, String, bool), rpc::RpcError>;

/// Delivers a streamed message; the stream then follows its task until it settles.
async fn start_streaming(served: Served<'_>, params: &Value) -> Started {
    let inbound = methods::inbound(params)?;
    let summary = methods::deliver(served, &inbound)
        .await
        .map_err(|refusal| rpc::RpcError::refused("SendStreamingMessage", &refusal))?;
    let task = summary
        .get("task_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(("SendStreamingMessage", task.to_owned(), true))
}

/// Checks a subscription names a task that has not ended; the stream follows it until it
/// does.
async fn start_subscription(served: Served<'_>, params: &Value) -> Started {
    let task = rpc::required(params, "id")?;
    let details = served
        .backend
        .details(served.route, task)
        .await
        .map_err(|refusal| rpc::RpcError::refused("SubscribeToTask", &refusal))?;
    let state = details
        .pointer("/task/state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if super::backend::ended(state) {
        return Err(rpc::RpcError::unsupported("the task has already ended"));
    }
    Ok(("SubscribeToTask", task.to_owned(), false))
}

async fn local_artifact(
    State(shared): State<Shared>,
    Path((agent, task, name)): Path<(String, String, String)>,
) -> Response {
    match local(&agent) {
        Some(route) => artifact::serve(shared.backend, (route, task, name)).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn remote_artifact(
    State(shared): State<Shared>,
    Path((endpoint, agent, task, name)): Path<(String, String, String, String)>,
) -> Response {
    match remote(&endpoint, &agent) {
        Some(route) => artifact::serve(shared.backend, (route, task, name)).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// An empty body with `status`.
pub(super) fn status(status: StatusCode) -> Response {
    (status, Body::empty()).into_response()
}
