mod assets;
mod auth;
mod csrf;
mod headers;
mod rate_limit;
mod server;
mod types;

use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse as _, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

pub use assets::WebAssets;
use assets::asset_response;
pub use auth::WebAuthService;
use rate_limit::LoginRateLimit;
pub use server::LoopbackWebServer;
pub use types::{
    AuthFailure, AuthenticatedSession, Clock, CredentialState, LoginSession, PasswordAction,
    SystemClock, WebAuthConfig,
};

const SESSION_COOKIE: &str = "ma2a_session";
const CSRF_COOKIE: &str = "ma2a_csrf";

/// Bounded loopback HTTP server settings.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct WebServerConfig {
    request_timeout: Duration,
}

impl WebServerConfig {
    /// Maximum accepted request body.
    pub const MAX_BODY_BYTES: usize = 16_384;
    /// Login attempts accepted per fixed rolling window.
    pub const LOGIN_ATTEMPTS: usize = 5;
    const LOGIN_WINDOW_MS: i64 = 60_000;

    /// Overrides the per-request deadline.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl Default for WebServerConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
struct WebState {
    auth: WebAuthService,
    assets: WebAssets,
    config: WebServerConfig,
    port: u16,
    login_rate: std::sync::Arc<LoginRateLimit>,
}

/// Builds the loopback router for a selected listener port.
#[expect(
    clippy::too_many_arguments,
    reason = "the public testable router seam mirrors its four independent construction inputs"
)]
pub fn build_router(
    auth: WebAuthService,
    assets: WebAssets,
    config: WebServerConfig,
    port: u16,
) -> Router {
    let state = WebState {
        auth,
        assets,
        config,
        port,
        login_rate: std::sync::Arc::new(LoginRateLimit::new(
            WebServerConfig::LOGIN_ATTEMPTS,
            WebServerConfig::LOGIN_WINDOW_MS,
        )),
    };
    Router::new()
        .route("/api/v1/web/auth/state", get(auth_state))
        .route("/api/v1/web/auth/login", post(login))
        .route("/api/v1/web/auth/logout", post(logout))
        .route("/api/v1/web/session/touch", post(touch_session))
        .fallback(asset)
        .layer(DefaultBodyLimit::max(WebServerConfig::MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            headers::request_guards,
        ))
        .with_state(state)
}

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
    if !headers::valid_same_origin(&headers, state.port) {
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
    if !headers::valid_same_origin(&headers, state.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let session = match authenticated_mutation(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.auth.logout(session).await {
        Ok(()) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
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
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn touch_session(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !headers::valid_same_origin(&headers, state.port) {
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
