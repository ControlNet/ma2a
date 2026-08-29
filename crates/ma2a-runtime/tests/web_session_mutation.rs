//! Authenticated loopback Web session mutation integration coverage.

#[path = "support/web.rs"]
mod support;

use std::sync::Arc;

use ma2a_runtime::web::{PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use rusqlite::Connection;
use support::{RunningServer, TempState, TestResult, clock, password, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_session_touch_requires_matching_csrf() -> TestResult {
    // Given
    let state = TempState::new("session-touch")?;
    let config = StoreConfig::new(state.path());
    let clock = clock(2_500);
    let auth = WebAuthService::open(
        config.clone(),
        Arc::clone(&clock) as Arc<dyn ma2a_runtime::web::Clock>,
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
    clock.set(3_500);
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
    let accepted_expiry = Connection::open(config.database_path())?.query_row(
        "SELECT idle_expires_at_ms FROM sessions",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    clock.set(4_500);
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
    let denied_expiry = Connection::open(config.database_path())?.query_row(
        "SELECT idle_expires_at_ms FROM sessions",
        [],
        |row| row.get::<_, i64>(0),
    )?;

    // Then
    assert_eq!(accepted.status, 204);
    assert_eq!(denied.status, 403);
    assert_eq!(denied_expiry, accepted_expiry);
    server.stop().await
}
