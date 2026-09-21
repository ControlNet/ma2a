mod assets;
mod auth;
mod csrf;
mod headers;
mod lifecycle;
mod rate_limit;
mod router;
mod runtime_mutations;
mod runtime_routes;
mod server;
mod types;

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

pub use assets::WebAssets;
use assets::asset_response;
pub use auth::WebAuthService;
pub use lifecycle::WebLifecycle;
use router::WebState;
pub use router::{WebRuntimeDependencies, WebServerConfig, build_router, build_runtime_router};
pub use server::LoopbackWebServer;
pub use types::{
    AuthFailure, AuthenticatedSession, Clock, CredentialState, LoginSession, PasswordAction,
    SystemClock, WebAuthConfig,
};

const SESSION_COOKIE: &str = "ma2a_session";
const CSRF_COOKIE: &str = "ma2a_csrf";

async fn auth_state(State(state): State<WebState>) -> Response {
    state.auth.setup_required().await.map_or_else(
        |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        |required| {
            Json(json!({
                "state": if required { "setup_required" } else { "configured" }
            }))
            .into_response()
        },
    )
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

async fn login(State(state): State<WebState>, headers: HeaderMap, body: Bytes) -> Response {
    if !headers::valid_same_origin(&headers, &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !state.login_rate.allow(state.auth.now_ms()) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(request) = serde_json::from_slice::<LoginRequest>(&body) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    match state.auth.login(Zeroizing::new(request.password)).await {
        Ok(session) => {
            let max_age = WebAuthConfig::ABSOLUTE_TIMEOUT_MS / 1_000;
            let cookie = format!(
                "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}",
                session.bearer()
            );
            let csrf_cookie = format!(
                "{CSRF_COOKIE}={}; Path=/; SameSite=Strict; Max-Age={max_age}",
                session.csrf_token()
            );
            let mut response = Json(json!({ "csrf_token": session.csrf_token() })).into_response();
            if let Ok(value) = csrf_cookie.parse() {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            if let Ok(value) = cookie.parse() {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            response
        }
        Err(AuthFailure::Unauthorized | AuthFailure::Conflict) => {
            StatusCode::UNAUTHORIZED.into_response()
        }
        Err(AuthFailure::Internal) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn logout(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !headers::valid_same_origin(&headers, &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let session = match authenticated_mutation(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.auth.logout(session).await {
        Ok(()) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            expire_session_cookies(&mut response);
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(super) fn expire_session_cookies(response: &mut Response) {
    response.headers_mut().append(
        header::SET_COOKIE,
        header::HeaderValue::from_static("ma2a_csrf=; Path=/; SameSite=Strict; Max-Age=0"),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        header::HeaderValue::from_static(
            "ma2a_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
}

async fn touch_session(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !headers::valid_same_origin(&headers, &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match authenticated_mutation(&state, &headers).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => response,
    }
}

async fn authenticated_mutation(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<AuthenticatedSession, Response> {
    let bearer = cookie(headers).ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
    let csrf =
        headers::single_header_value(headers, &header::HeaderName::from_static("x-csrf-token"))
            .ok_or_else(|| StatusCode::FORBIDDEN.into_response())?;
    state
        .auth
        .authenticate_mutation(bearer, csrf)
        .await
        .map_err(|_| StatusCode::FORBIDDEN.into_response())
}

async fn asset(State(state): State<WebState>, request: axum::extract::Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if path.contains('.') {
        return asset_response(&state.assets, path);
    }
    let Ok(setup_required) = state.auth.setup_required().await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if setup_required {
        return if path == "setup" {
            asset_response(&state.assets, "index.html")
        } else {
            Redirect::temporary("/setup").into_response()
        };
    }
    if path == "setup" {
        return Redirect::temporary("/login").into_response();
    }
    if path != "login" {
        let authenticated = if let Some(bearer) = cookie(request.headers()) {
            state.auth.authenticate(bearer).await.is_ok()
        } else {
            false
        };
        if !authenticated {
            return Redirect::temporary("/login").into_response();
        }
    }
    let requested = if path.is_empty() || !path.contains('.') {
        "index.html"
    } else {
        path
    };
    asset_response(&state.assets, requested)
}

fn cookie(headers: &HeaderMap) -> Option<&str> {
    let value = headers::single_header_value(headers, &header::COOKIE)?;
    let mut bearer = None;
    for part in value.split(';').map(str::trim) {
        let (name, token) = part.split_once('=')?;
        if name == SESSION_COOKIE {
            if token.is_empty() || bearer.is_some() {
                return None;
            }
            bearer = Some(token);
        }
    }
    bearer
}
