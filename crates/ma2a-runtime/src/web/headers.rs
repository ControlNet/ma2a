use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse as _, Response},
};

use super::WebState;

#[expect(
    clippy::too_many_arguments,
    reason = "Axum middleware injects state, peer metadata, request, and continuation separately"
)]
pub(super) async fn request_guards(
    axum::extract::State(state): axum::extract::State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !peer.ip().is_loopback() || !valid_host(request.headers(), state.port) {
        return secured(StatusCode::FORBIDDEN.into_response());
    }
    if state.config.request_timeout.is_zero() {
        return secured(StatusCode::REQUEST_TIMEOUT.into_response());
    }
    tokio::time::timeout(state.config.request_timeout, next.run(request))
        .await
        .map_or_else(
            |_| secured(StatusCode::REQUEST_TIMEOUT.into_response()),
            secured,
        )
}

pub(super) fn valid_same_origin(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = single_header_value(headers, &header::HOST) else {
        return false;
    };
    let Some(origin) = single_header_value(headers, &header::ORIGIN) else {
        return false;
    };
    let fetch_site = single_header_value(headers, &HeaderName::from_static("sec-fetch-site"));
    valid_host_value(host, port)
        && origin == format!("http://{host}")
        && fetch_site == Some("same-origin")
}

fn valid_host(headers: &HeaderMap, port: u16) -> bool {
    single_header_value(headers, &header::HOST).is_some_and(|host| valid_host_value(host, port))
}

pub(super) fn single_header_value<'a>(
    headers: &'a HeaderMap,
    name: &HeaderName,
) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn valid_host_value(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("[::1]:{port}")
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
