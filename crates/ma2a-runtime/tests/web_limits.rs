//! Loopback HTTP request-limit integration coverage.

#[path = "support/web.rs"]
mod support;

use std::time::Duration;

use ma2a_runtime::web::{PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock, password, request};

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
