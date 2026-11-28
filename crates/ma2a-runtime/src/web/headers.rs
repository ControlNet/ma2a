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
        .is_none_or(|binding| binding.address.ip().to_canonical().is_loopback());
    if (loopback_only && !peer.ip().to_canonical().is_loopback())
        || !valid_host(request.headers(), &state)
    {
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
    let binding_ip = binding.address.ip().to_canonical();
    // A wildcard listener accepts requests through any interface or DNS name.
    // An explicit binding accepts its configured hostname and resolved address.
    binding_ip.is_unspecified()
        || name.eq_ignore_ascii_case(&binding.host)
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.to_canonical() == binding_ip)
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

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use axum::{
        body::Body,
        extract::connect_info::MockConnectInfo,
        http::{Request, StatusCode},
    };
    use ma2a_store::StoreConfig;
    use tower::ServiceExt as _;

    use crate::web::{
        SystemClock, WebAssets, WebAuthConfig, WebAuthService, WebServerConfig, build_router,
        router::WebBinding,
    };

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn send_guard_request(
        auth: WebAuthService,
        binding: Option<(&str, &str)>,
        (peer, host): (&str, &str),
    ) -> Result<StatusCode, Box<dyn std::error::Error + Send + Sync>> {
        let port = 43_210;
        let binding = binding
            .map(|(bind_host, addr)| {
                Ok::<_, std::net::AddrParseError>(Arc::new(WebBinding {
                    host: bind_host.to_owned(),
                    address: addr.parse()?,
                }))
            })
            .transpose()?;
        let config = WebServerConfig {
            binding,
            ..WebServerConfig::default()
        };
        let router = build_router(auth, WebAssets::new(&[]), config, port)
            .layer(MockConnectInfo(peer.parse::<SocketAddr>()?));
        let request = Request::builder()
            .uri("/api/v1/web/auth/state")
            .header("host", host)
            .body(Body::empty())?;
        Ok(router.oneshot(request).await?.status())
    }

    #[tokio::test]
    async fn mapped_ipv4_addresses_canonicalize_in_host_and_loopback_guards()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = TempDir(std::env::temp_dir().join(format!(
            "ma2a-web-headers-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir(&dir.0)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700))?;
        }
        let auth = WebAuthService::open(
            StoreConfig::new(&dir.0),
            Arc::new(SystemClock),
            WebAuthConfig::default(),
        )
        .await?;

        let cases = [
            (
                Some(("0.0.0.0", "[::ffff:0.0.0.0]:43210")),
                "127.0.0.1:12345",
                "127.0.0.1:43210",
                StatusCode::OK,
            ),
            (
                Some(("0.0.0.0", "[::ffff:0.0.0.0]:43210")),
                "127.0.0.1:12345",
                "example.test:43210",
                StatusCode::OK,
            ),
            (
                Some(("mapped-loopback", "[::ffff:127.0.0.1]:43210")),
                "[::ffff:127.0.0.1]:12345",
                "127.0.0.1:43210",
                StatusCode::OK,
            ),
            (
                Some(("ipv4-loopback", "127.0.0.1:43210")),
                "[::ffff:127.0.0.1]:12345",
                "[::ffff:127.0.0.1]:43210",
                StatusCode::OK,
            ),
            (
                Some(("mapped-loopback", "[::ffff:127.0.0.1]:43210")),
                "192.0.2.1:12345",
                "mapped-loopback:43210",
                StatusCode::FORBIDDEN,
            ),
            (
                None,
                "[::ffff:127.0.0.1]:12345",
                "127.0.0.1:43210",
                StatusCode::OK,
            ),
            (
                None,
                "[::ffff:192.0.2.1]:12345",
                "127.0.0.1:43210",
                StatusCode::FORBIDDEN,
            ),
        ];
        for (binding, peer, host, expected) in cases {
            assert_eq!(
                send_guard_request(auth.clone(), binding, (peer, host)).await?,
                expected,
                "binding={binding:?}, peer={peer}, host={host}"
            );
        }
        Ok(())
    }
}
