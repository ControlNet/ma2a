//! Ambiguous HTTP security metadata rejection coverage.

#[path = "support/web.rs"]
mod support;

use ma2a_runtime::web::{PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock, password, request};

#[tokio::test]
async fn duplicate_security_headers_and_session_cookies_fail_closed() -> TestResult {
    let state = TempState::new("header-ambiguity")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(6_000),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let session = auth.login(password()).await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());

    let duplicate_host = server
        .request(
            format!(
                "GET /api/v1/web/auth/state HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                server.port(),
                server.port()
            )
            .as_bytes(),
        )
        .await?;
    let duplicate_origin = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[
                ("Origin", &origin),
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
            ],
            b"{}",
        ))
        .await?;
    let duplicate_fetch_site = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/login",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Sec-Fetch-Site", "same-origin"),
            ],
            b"{}",
        ))
        .await?;
    let cookie = format!("ma2a_session={}", session.bearer());
    let duplicate_cookie = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/logout",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-Csrf-Token", session.csrf_token()),
                ("Cookie", &cookie),
                ("Cookie", &cookie),
            ],
            b"",
        ))
        .await?;
    let duplicate_csrf = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/session/touch",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-Csrf-Token", session.csrf_token()),
                ("X-Csrf-Token", session.csrf_token()),
                ("Cookie", &cookie),
            ],
            b"",
        ))
        .await?;

    assert!(duplicate_host.status >= 400);
    assert_eq!(duplicate_origin.status, 403);
    assert_eq!(duplicate_fetch_site.status, 403);
    assert_eq!(duplicate_cookie.status, 401);
    assert_eq!(duplicate_csrf.status, 403);
    server.stop().await
}
