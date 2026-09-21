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
    let loopback_only = state
        .config
        .binding
        .as_ref()
        .is_none_or(|binding| binding.address.ip().is_loopback());
    if (loopback_only && !peer.ip().is_loopback()) || !valid_host(request.headers(), &state) {
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

pub(super) fn valid_same_origin(headers: &HeaderMap, state: &WebState) -> bool {
    let Some(host) = single_header_value(headers, &header::HOST) else {
        return false;
    };
    let Some(origin) = single_header_value(headers, &header::ORIGIN) else {
        return false;
    };
    let fetch_site_name = HeaderName::from_static("sec-fetch-site");
    let fetch_site = single_header_value(headers, &fetch_site_name);
    // HTTP URLs outside trusted loopback contexts may omit Fetch Metadata.
    // Origin remains mandatory; duplicate or cross-site metadata still fails closed.
    let valid_fetch_site = fetch_site == Some("same-origin")
        || (state.config.binding.is_some() && !headers.contains_key(&fetch_site_name));
    valid_host_value(host, state) && origin == format!("http://{host}") && valid_fetch_site
}

fn valid_host(headers: &HeaderMap, state: &WebState) -> bool {
    single_header_value(headers, &header::HOST).is_some_and(|host| valid_host_value(host, state))
}

pub(super) fn single_header_value<'a>(
    headers: &'a HeaderMap,
    name: &HeaderName,
) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn valid_host_value(host: &str, state: &WebState) -> bool {
    let Some(binding) = &state.config.binding else {
        return host == format!("127.0.0.1:{}", state.port)
            || host == format!("[::1]:{}", state.port);
    };
    if host.contains('@') {
        return false;
    }
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if (authority.port().is_some() && authority.port_u16().is_none())
        || authority.port_u16().unwrap_or(80) != state.port
    {
        return false;
    }
    let name = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if name.is_empty() {
        return false;
    }
    // A wildcard listener accepts requests through any interface or DNS name.
    // An explicit binding accepts its configured hostname and resolved address.
    binding.address.ip().is_unspecified()
        || name.eq_ignore_ascii_case(&binding.host)
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip == binding.address.ip())
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
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}
