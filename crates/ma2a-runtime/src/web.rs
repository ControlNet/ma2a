mod assets;
mod auth;
mod csrf;
mod gateway;
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
    body::{Body, to_bytes},
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
pub use lifecycle::{WebLifecycle, serve_connections};
use router::WebState;
pub use router::{WebRuntimeDependencies, WebServerConfig, build_router, build_runtime_router};
pub use server::LoopbackWebServer;
pub use types::{
    AuthFailure, AuthenticatedSession, Clock, CredentialState, LoginSession, PasswordAction,
    SystemClock, WebAuthConfig,
};

async fn auth_state(State(state): State<WebState>) -> Response {
    state.auth.setup_required().await.map_or_else(
        |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        |required| {
            Json(json!({
                "state": if required { "setup_required" } else { "configured" },
                "csrf_cookie": state.auth.csrf_cookie_name(),
            }))
            .into_response()
        },
    )
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

/// Refuses on headers alone before reading the body, so a client that never sends the body it
/// announced holds a connection only after spending a login attempt.
async fn login(State(state): State<WebState>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    if !headers::valid_same_origin(&parts.headers, &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !state.login_rate.allow(state.auth.now_ms()) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(body) = to_bytes(body, WebServerConfig::MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(request) = serde_json::from_slice::<LoginRequest>(&body) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    match state.auth.login(Zeroizing::new(request.password)).await {
        Ok(session) => {
            let mut response = Json(json!({
                "csrf_token": session.csrf_token(),
                "csrf_cookie": state.auth.csrf_cookie_name(),
            }))
            .into_response();
            append_session_cookies(&state, &mut response, Some(&session));
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
            expire_session_cookies(&state, &mut response);
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn expire_session_cookies(state: &WebState, response: &mut Response) {
    append_session_cookies(state, response, None);
}

fn append_session_cookies(
    state: &WebState,
    response: &mut Response,
    session: Option<&LoginSession>,
) {
    let (bearer, csrf_token, max_age) = session.map_or(("", "", 0), |session| {
        (
            session.bearer(),
            session.csrf_token(),
            WebAuthConfig::ABSOLUTE_TIMEOUT_MS / 1_000,
        )
    });
    let csrf = format!(
        "{}={csrf_token}; Path=/; SameSite=Strict; Max-Age={max_age}",
        state.auth.csrf_cookie_name()
    );
    let session = format!(
        "{}={bearer}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}",
        state.auth.session_cookie_name()
    );
    for header_line in [csrf, session] {
        if let Ok(value) = header_line.parse() {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
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
    let bearer = cookie(state, headers).ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
    let csrf =
        headers::single_header_value(headers, &header::HeaderName::from_static("x-csrf-token"))
            .ok_or_else(|| StatusCode::FORBIDDEN.into_response())?;
    // An internal failure is not a refusal: the session may still be valid, and a browser that
    // read it as signed out would show a login page for a live session. A refusal answers 401
    // only when the session itself is gone; a live session whose request was refused, as with a
    // stale CSRF token, answers 403. The session is looked up without being touched.
    let refused = match state.auth.authenticate_mutation(bearer, csrf).await {
        Ok(session) => return Ok(session),
        Err(AuthFailure::Internal) => return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(AuthFailure::Unauthorized | AuthFailure::Conflict) => state.auth.validate(bearer).await,
    };
    Err(match refused {
        Ok(_) => StatusCode::FORBIDDEN,
        Err(AuthFailure::Internal) => StatusCode::INTERNAL_SERVER_ERROR,
        Err(AuthFailure::Unauthorized | AuthFailure::Conflict) => StatusCode::UNAUTHORIZED,
    }
    .into_response())
}

async fn asset(State(state): State<WebState>, request: axum::extract::Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if path.contains('.') && path != "index.html" {
        return asset_response(&state.assets, path);
    }
    let Ok(setup_required) = state.auth.setup_required().await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if setup_required {
        return if path == "setup" {
            index_response(&state).await
        } else {
            Redirect::temporary("/setup").into_response()
        };
    }
    if path == "setup" {
        return Redirect::temporary("/login").into_response();
    }
    if path != "login" {
        match authenticated_read(&state, request.headers()).await {
            Ok(()) => {}
            Err(StatusCode::UNAUTHORIZED) => return Redirect::temporary("/login").into_response(),
            Err(status) => return status.into_response(),
        }
    }
    index_response(&state).await
}

async fn index_response(state: &WebState) -> Response {
    let response = asset_response(&state.assets, "index.html");
    if response.status() == StatusCode::OK {
        return with_csrf_cookie_meta(response, state.auth.csrf_cookie_name()).await;
    }
    response
}

async fn with_csrf_cookie_meta(response: Response, csrf_cookie: &str) -> Response {
    let (parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, 1_048_576).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Some(html) = std::str::from_utf8(&bytes)
        .ok()
        .filter(|text| text.contains("</head>"))
    else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let injected = html.replacen(
        "</head>",
        &format!("<meta name=\"ma2a-csrf-cookie\" content=\"{csrf_cookie}\" /></head>"),
        1,
    );
    Response::from_parts(parts, Body::from(injected))
}

/// Authenticates a read. Only a request without a live session is 401; an internal failure is
/// 500, so a browser never signs out a session that is still valid.
async fn authenticated_read(state: &WebState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let bearer = cookie(state, headers).ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .auth
        .authenticate(bearer)
        .await
        .map(drop)
        .map_err(read_status)
}

/// The status a read answers when its session check fails.
const fn read_status(failure: AuthFailure) -> StatusCode {
    match failure {
        AuthFailure::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        AuthFailure::Unauthorized | AuthFailure::Conflict => StatusCode::UNAUTHORIZED,
    }
}

fn cookie<'a>(state: &WebState, headers: &'a HeaderMap) -> Option<&'a str> {
    let expected = state.auth.session_cookie_name();
    let mut bearer = None;
    for raw in headers.get_all(header::COOKIE) {
        let value = raw.to_str().ok()?;
        for part in value
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let Some((name, token)) = part.split_once('=') else {
                continue;
            };
            if name == expected {
                if token.is_empty() || bearer.is_some() {
                    return None;
                }
                bearer = Some(token);
            }
        }
    }
    bearer
}
