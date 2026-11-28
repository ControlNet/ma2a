//! A refused authenticated request tells a live session from one that is gone.

#[path = "support/web.rs"]
mod support;

use ma2a_runtime::web::{PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock, password, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_logout_answers_403_while_the_session_lives_and_401_once_it_is_gone() -> TestResult
{
    // Given
    let state = TempState::new("logout-refusal")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_500),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let session = auth.login(password()).await?;
    let cookie = format!("{}={}", auth.session_cookie_name(), session.bearer());
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;
    let origin = format!("http://127.0.0.1:{}", server.port());
    let old_token = "ab".repeat(32);
    let logout = |token: &str| {
        request(
            server.port(),
            "POST",
            "/api/v1/web/auth/logout",
            &[
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Cookie", &cookie),
                ("X-CSRF-Token", token),
            ],
            b"",
        )
    };

    // When
    let refused = server.request(&logout(&old_token)).await?;
    let accepted = server.request(&logout(session.csrf_token())).await?;
    let gone = server.request(&logout(session.csrf_token())).await?;

    // Then: a stale token leaves the live session in place; a logged-out one is reported gone.
    assert_eq!(refused.status, 403);
    assert!(refused.set_cookies.is_empty());
    assert_eq!(accepted.status, 204);
    assert_eq!(gone.status, 401);
    server.stop().await
}
