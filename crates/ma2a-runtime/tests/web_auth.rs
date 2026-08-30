//! Web credential and session lifecycle integration coverage.

#[path = "support/web.rs"]
mod support;

use std::sync::Arc;

use ma2a_runtime::web::{
    AuthFailure, PasswordAction, WebAuthConfig, WebAuthService, WebServerConfig,
};
use ma2a_store::StoreConfig;
use rusqlite::Connection;
use subtle::ConstantTimeEq as _;
use support::{RunningServer, TempState, TestResult, clock, password, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_password_state_exposes_setup_without_a_browser_mutation() -> TestResult {
    let state = TempState::new("no-password")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_000),
        WebAuthConfig::default(),
    )
    .await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;

    let status = server
        .request(&request(
            server.port(),
            "GET",
            "/api/v1/web/auth/state",
            &[],
            b"",
        ))
        .await?;
    let setup = server
        .request(&request(
            server.port(),
            "POST",
            "/api/v1/web/auth/setup",
            &[],
            b"{}",
        ))
        .await?;
    let script = server
        .request(&request(server.port(), "GET", "/assets/app.js", &[], b""))
        .await?;

    assert_eq!(status.status, 200);
    assert!(
        status
            .body
            .windows(14)
            .any(|value| value == b"setup_required")
    );
    assert_eq!(setup.status, 404);
    assert_eq!(script.status, 200);
    assert_eq!(
        script.headers.get("content-type").map(String::as_str),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        script.headers.get("cache-control").map(String::as_str),
        Some("public, max-age=31536000, immutable")
    );
    server.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_password_reset_increments_epoch_and_revokes_old_cookie() -> TestResult {
    let state = TempState::new("reset-revokes")?;
    let config = StoreConfig::new(state.path());
    let auth = WebAuthService::open(config.clone(), clock(5_000), WebAuthConfig::default()).await?;
    let first_state = auth
        .change_password(PasswordAction::Set, password())
        .await?;
    let login = auth.login(password()).await?;

    let second_state = auth
        .change_password(PasswordAction::Reset, password())
        .await?;
    let old_session = auth.authenticate(login.bearer()).await;

    assert_eq!(first_state.auth_epoch() + 1, second_state.auth_epoch());
    assert!(matches!(old_session, Err(AuthFailure::Unauthorized)));
    let connection = Connection::open(config.database_path())?;
    let verifier = connection.query_row(
        "SELECT password_verifier FROM ui_credentials WHERE singleton = 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    assert!(verifier.starts_with(b"$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(!contains(&verifier, password().as_bytes()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_rotates_bearer_and_database_contains_only_digests() -> TestResult {
    let state = TempState::new("digest-only")?;
    let config = StoreConfig::new(state.path());
    let auth =
        WebAuthService::open(config.clone(), clock(10_000), WebAuthConfig::default()).await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;

    let first = auth.login(password()).await?;
    let second = auth.login(password()).await?;

    let first_digest = blake3::hash(first.bearer().as_bytes());
    let second_digest = blake3::hash(second.bearer().as_bytes());
    assert!(bool::from(
        first_digest.as_bytes().ct_ne(second_digest.as_bytes())
    ));
    let database = std::fs::read(config.database_path())?;
    assert!(!contains(&database, first.bearer().as_bytes()));
    assert!(!contains(&database, first.csrf_token().as_bytes()));
    assert!(!contains(&database, second.bearer().as_bytes()));
    assert!(!contains(&database, second.csrf_token().as_bytes()));
    assert!(matches!(
        auth.authenticate(&first.bearer().to_ascii_uppercase())
            .await,
        Err(AuthFailure::Unauthorized)
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_and_absolute_expiry_are_deterministic_without_sleeps() -> TestResult {
    let state = TempState::new("expiry")?;
    let clock = clock(20_000);
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        Arc::clone(&clock) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(PasswordAction::Set, password())
        .await?;
    let idle = auth.login(password()).await?;

    clock.set(20_000 + WebAuthConfig::IDLE_TIMEOUT_MS + 1);
    let idle_result = auth.authenticate(idle.bearer()).await;
    clock.set(30_000);
    let absolute = auth.login(password()).await?;
    clock.set(30_000 + WebAuthConfig::ABSOLUTE_TIMEOUT_MS + 1);
    let absolute_result = auth.authenticate(absolute.bearer()).await;

    assert!(matches!(idle_result, Err(AuthFailure::Unauthorized)));
    assert!(matches!(absolute_result, Err(AuthFailure::Unauthorized)));
    Ok(())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}
