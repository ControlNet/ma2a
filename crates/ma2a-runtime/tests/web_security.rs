//! Loopback HTTP request-guard and response-hardening integration coverage.

#[path = "support/web.rs"]
mod support;

use std::{net::SocketAddr, time::Duration};

use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_runtime::web::{
    PasswordAction, WebAssets, WebAuthConfig, WebAuthService, WebServerConfig, build_router,
};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock, password, request};
use tower::ServiceExt as _;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_cookie_and_security_headers_are_strict() -> TestResult {
    let state = TempState::new("cookie-headers")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_000),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let body = serde_json::to_vec(&serde_json::json!({"password": password().as_str()}))?;

    let response = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Content-Type", "application/json"),
            ],
            &body,
        ))
        .await?;

    assert_eq!(response.status, 200);
    let session_cookie = response
        .set_cookies
        .iter()
        .find(|cookie| cookie.starts_with("ma2a_session="))
        .ok_or("missing session cookie")?;
    let csrf_cookie = response
        .set_cookies
        .iter()
        .find(|cookie| cookie.starts_with("ma2a_csrf="))
        .ok_or("missing CSRF cookie")?;
    assert!(session_cookie.contains("; Path=/; HttpOnly; SameSite=Strict; Max-Age="));
    assert!(!session_cookie.contains("Domain="));
    assert!(!session_cookie.contains("; Secure"));
    assert!(csrf_cookie.contains("; Path=/; SameSite=Strict; Max-Age="));
    assert!(!csrf_cookie.contains("HttpOnly"));
    assert!(!csrf_cookie.contains("Domain="));
    assert_eq!(
        response
            .headers
            .get("x-content-type-options")
            .map(String::as_str),
        Some("nosniff")
    );
    assert_eq!(
        response.headers.get("x-frame-options").map(String::as_str),
        Some("DENY")
    );
    assert_eq!(
        response.headers.get("referrer-policy").map(String::as_str),
        Some("no-referrer")
    );
    assert_eq!(
        response.headers.get("cache-control").map(String::as_str),
        Some("no-store")
    );
    assert!(response.headers.contains_key("content-security-policy"));
    assert!(!response.headers.contains_key("access-control-allow-origin"));
    server.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_expires_session_and_csrf_cookies() -> TestResult {
    // Given
    let state = TempState::new("logout-cookies")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_500),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let session = auth.login(password()).await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let cookie = format!("ma2a_session={}", session.bearer());

    // When
    let response = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/logout",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Cookie", &cookie),
                ("X-CSRF-Token", session.csrf_token()),
            ],
            b"",
        ))
        .await?;

    // Then
    assert_eq!(response.status, 204);
    assert!(
        response.set_cookies.iter().any(|cookie| {
            cookie == "ma2a_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
        })
    );
    assert!(
        response
            .set_cookies
            .iter()
            .any(|cookie| { cookie == "ma2a_csrf=; Path=/; SameSite=Strict; Max-Age=0" })
    );
    server.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_origin_fetch_site_csrf_and_malformed_cookie_fail_closed() -> TestResult {
    let state = TempState::new("request-guards")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(2_000),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let login = auth.login(password()).await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let cookie = format!("ma2a_session={}", login.bearer());

    let wrong_host = server
        .request(b"GET /api/v1/web/auth/state HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n")
        .await?;
    let missing_origin = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[("Sec-Fetch-Site", "same-origin")],
            b"{}",
        ))
        .await?;
    let cross_site = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[("Origin", &origin), ("Sec-Fetch-Site", "cross-site")],
            b"{}",
        ))
        .await?;
    let no_csrf = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/logout",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Cookie", &cookie),
            ],
            b"",
        ))
        .await?;
    let malformed = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/logout",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Cookie", "ma2a_session"),
            ],
            b"",
        ))
        .await?;

    assert_eq!(wrong_host.status, 403);
    assert_eq!(missing_origin.status, 403);
    assert_eq!(cross_site.status, 403);
    assert_eq!(no_csrf.status, 403);
    assert_eq!(malformed.status, 401);
    server.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_body_deadline_and_rate_state_are_bounded() -> TestResult {
    let state = TempState::new("limits")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(3_000),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let server = RunningServer::start(auth.clone(), WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let headers = [
        ("Origin", origin.as_str()),
        ("Sec-Fetch-Site", "same-origin"),
        ("Content-Type", "application/json"),
    ];

    let oversized = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &headers,
            &vec![b'x'; WebServerConfig::MAX_BODY_BYTES + 1],
        ))
        .await?;
    let mut last_status = 0;
    for _attempt in 0..=WebServerConfig::LOGIN_ATTEMPTS {
        last_status = server
            .request(&request(
                server.port(),
                "POST",
                "/api/v1/web/auth/login",
                &headers,
                b"{}",
            ))
            .await?
            .status;
    }
    server.stop().await?;
    let timeout_server = RunningServer::start(
        auth,
        WebServerConfig::default().with_request_timeout(Duration::ZERO),
    )
    .await?;
    let timeout_origin = format!("http://127.0.0.1:{}", timeout_server.port());
    let timed_out = timeout_server
        .request(&request(
            timeout_server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[
                ("Origin", &timeout_origin),
                ("Sec-Fetch-Site", "same-origin"),
            ],
            b"{}",
        ))
        .await?;

    assert_eq!(oversized.status, 413);
    assert_eq!(last_status, 429);
    assert_eq!(timed_out.status, 408);
    timeout_server.stop().await
}

#[tokio::test]
async fn fabricated_non_loopback_accepted_peer_is_rejected() -> TestResult {
    let state = TempState::new("peer")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(4_000),
        WebAuthConfig::default(),
    )
    .await?;
    let router = build_router(
        auth,
        WebAssets::new(&[("index.html", b"ok")]),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 1], 12_345))));
    let request = Request::builder()
        .uri("/api/v1/web/auth/state")
        .header("host", "127.0.0.1:43210")
        .body(Body::empty())?;

    let response = router.oneshot(request).await?;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    Ok(())
}
