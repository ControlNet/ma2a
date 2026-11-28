//! Loopback HTTP request-guard and response-hardening integration coverage.

#[path = "support/web.rs"]
mod support;

use std::net::SocketAddr;

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
    let auth_session_cookie = auth.session_cookie_name().to_owned();
    let auth_csrf_cookie = auth.csrf_cookie_name().to_owned();
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
        .find(|cookie| cookie.starts_with(&format!("{auth_session_cookie}=")))
        .ok_or("missing session cookie")?;
    let csrf_cookie = response
        .set_cookies
        .iter()
        .find(|cookie| cookie.starts_with(&format!("{auth_csrf_cookie}=")))
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
    let session_cookie_name = auth.session_cookie_name().to_owned();
    let csrf_cookie_name = auth.csrf_cookie_name().to_owned();
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let cookie = format!("{session_cookie_name}={}", session.bearer());

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
    assert!(response.set_cookies.iter().any(|cookie| {
        cookie == &format!("{session_cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    }));
    assert!(response.set_cookies.iter().any(|cookie| {
        cookie == &format!("{csrf_cookie_name}=; Path=/; SameSite=Strict; Max-Age=0")
    }));
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
    let session_cookie_name = auth.session_cookie_name().to_owned();
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let cookie = format!("{session_cookie_name}={}", login.bearer());

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
                ("Cookie", &session_cookie_name),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_runtimes_on_same_host_scope_cookies_independently() -> TestResult {
    let state_a = TempState::new("scope-a")?;
    let state_b = TempState::new("scope-b")?;
    let server_a = start_configured_server(state_a.path()).await?;
    let server_b = start_configured_server(state_b.path()).await?;
    let login_body = serde_json::to_vec(&serde_json::json!({"password": password().as_str()}))?;

    let resp_a = server_a
        .request(&same_origin_post(
            server_a.port(),
            "/api/v1/web/auth/login",
            (None, None, &login_body),
        ))
        .await?;
    let resp_b = server_b
        .request(&same_origin_post(
            server_b.port(),
            "/api/v1/web/auth/login",
            (None, None, &login_body),
        ))
        .await?;
    assert_eq!((resp_a.status, resp_b.status), (200, 200));

    let (sess_name_a, sess_val_a) = cookie_pair_by_prefix(&resp_a.set_cookies, "ma2a_session")?;
    let (csrf_name_a, csrf_val_a) = cookie_pair_by_prefix(&resp_a.set_cookies, "ma2a_csrf")?;
    let (sess_name_b, sess_val_b) = cookie_pair_by_prefix(&resp_b.set_cookies, "ma2a_session")?;
    let (csrf_name_b, csrf_val_b) = cookie_pair_by_prefix(&resp_b.set_cookies, "ma2a_csrf")?;
    assert_ne!(sess_name_a, sess_name_b);
    assert_ne!(csrf_name_a, csrf_name_b);

    let jar = format!(
        "{sess_name_a}={sess_val_a}; {csrf_name_a}={csrf_val_a}; {sess_name_b}={sess_val_b}; {csrf_name_b}={csrf_val_b}"
    );
    assert_eq!(
        server_a
            .request(&same_origin_post(
                server_a.port(),
                "/api/v1/web/session/touch",
                (Some(&jar), Some(&csrf_val_a), b""),
            ))
            .await?
            .status,
        204
    );
    assert_eq!(
        server_b
            .request(&same_origin_post(
                server_b.port(),
                "/api/v1/web/session/touch",
                (Some(&jar), Some(&csrf_val_b), b""),
            ))
            .await?
            .status,
        204
    );

    let logout_b = server_b
        .request(&same_origin_post(
            server_b.port(),
            "/api/v1/web/auth/logout",
            (Some(&jar), Some(&csrf_val_b), b""),
        ))
        .await?;
    assert_eq!(logout_b.status, 204);
    assert!(
        logout_b
            .set_cookies
            .iter()
            .any(|c| c.starts_with(&format!("{sess_name_b}=;")))
    );
    assert!(
        !logout_b
            .set_cookies
            .iter()
            .any(|c| c.starts_with(&format!("{sess_name_a}=;")))
    );
    assert_eq!(
        server_a
            .request(&same_origin_post(
                server_a.port(),
                "/api/v1/web/session/touch",
                (Some(&jar), Some(&csrf_val_a), b""),
            ))
            .await?
            .status,
        204
    );
    server_a.stop().await?;
    server_b.stop().await
}

async fn start_configured_server(path: &std::path::Path) -> TestResult<RunningServer> {
    let auth = WebAuthService::open(
        StoreConfig::new(path),
        clock(1_000),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    RunningServer::start(auth, WebServerConfig::default()).await
}

fn cookie_pair_by_prefix(cookies: &[String], prefix: &str) -> TestResult<(String, String)> {
    let raw = cookies
        .iter()
        .find_map(|c| c.split(';').next().filter(|p| p.starts_with(prefix)))
        .ok_or_else(|| format!("missing cookie with prefix {prefix}"))?;
    let (name, val) = raw.split_once('=').ok_or("malformed cookie")?;
    Ok((name.to_owned(), val.to_owned()))
}

fn same_origin_post(
    port: u16,
    path: &str,
    (cookie, csrf, body): (Option<&str>, Option<&str>, &[u8]),
) -> Vec<u8> {
    let origin = format!("http://127.0.0.1:{port}");
    let mut headers = vec![
        ("Origin", origin.as_str()),
        ("Sec-Fetch-Site", "same-origin"),
    ];
    if let Some(cookie) = cookie {
        headers.push(("Cookie", cookie));
    }
    if let Some(csrf) = csrf {
        headers.push(("X-CSRF-Token", csrf));
    }
    if !body.is_empty() {
        headers.push(("Content-Type", "application/json"));
    }
    request(port, "POST", path, &headers, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn index_html_follows_setup_auth_routing_and_carries_csrf_meta() -> TestResult {
    let state = TempState::new("index-html-routing")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_000),
        WebAuthConfig::default(),
    )
    .await?;
    let csrf_cookie_name = auth.csrf_cookie_name().to_owned();
    let session_cookie_name = auth.session_cookie_name().to_owned();
    let expected_meta =
        format!("<meta name=\"ma2a-csrf-cookie\" content=\"{csrf_cookie_name}\" />");
    let server = RunningServer::start(auth.clone(), WebServerConfig::default()).await?;

    let index_during_setup = server
        .request(&request(server.port(), "GET", "/index.html", &[], b""))
        .await?;
    assert_eq!(index_during_setup.status, 307);
    assert_eq!(
        index_during_setup
            .headers
            .get("location")
            .map(String::as_str),
        Some("/setup")
    );

    let setup_page = server
        .request(&request(server.port(), "GET", "/setup", &[], b""))
        .await?;
    assert_eq!(setup_page.status, 200);
    assert!(
        String::from_utf8_lossy(&setup_page.body).contains(&expected_meta),
        "/setup omitted CSRF cookie meta"
    );

    auth.change_password(PasswordAction::Set, password())
        .await?;

    let unauth_index = server
        .request(&request(server.port(), "GET", "/index.html", &[], b""))
        .await?;
    assert_eq!(unauth_index.status, 307);
    assert_eq!(
        unauth_index.headers.get("location").map(String::as_str),
        Some("/login")
    );

    let login = auth.login(password()).await?;
    let cookie = format!("{session_cookie_name}={}", login.bearer());
    let auth_index = server
        .request(&request(
            server.port(),
            "GET",
            "/index.html",
            &[("Cookie", &cookie)],
            b"",
        ))
        .await?;
    assert_eq!(auth_index.status, 200);
    assert!(
        String::from_utf8_lossy(&auth_index.body).contains(&expected_meta),
        "/index.html omitted CSRF cookie meta"
    );

    server.stop().await
}
