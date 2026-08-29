//! Authenticated loopback Web session mutation integration coverage.

#[path = "support/web.rs"]
mod support;

use ma2a_runtime::web::{PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock, password, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_session_touch_requires_matching_csrf() -> TestResult {
    // Given
    let state = TempState::new("session-touch")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(2_500),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let login = auth.login(password()).await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let cookie = format!("ma2a_session={}", login.bearer());

    // When
    let accepted = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/session/touch",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-Csrf-Token", login.csrf_token()),
                ("Cookie", &cookie),
            ],
            b"",
        ))
        .await?;
    let denied = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/session/touch",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-Csrf-Token", "00"),
                ("Cookie", &cookie),
            ],
            b"",
        ))
        .await?;

    // Then
    assert_eq!(accepted.status, 204);
    assert_eq!(denied.status, 403);
    server.stop().await
}
