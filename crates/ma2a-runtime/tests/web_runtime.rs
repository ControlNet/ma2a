//! Authenticated Web projection integration coverage.

#[path = "support/web.rs"]
#[allow(
    dead_code,
    reason = "this integration target uses only the shared manual clock"
)]
mod support;

use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_runtime::{
    Runtime,
    api::Command,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig, build_runtime_router,
    },
};
use ma2a_store::StoreConfig;
use support::{TempState, clock};
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::too_many_lines,
    reason = "one live daemon scenario proves authentication, connection limits, permit release, revocation, and teardown"
)]
async fn authenticated_snapshot_and_stale_sse_use_the_daemon_projection() -> TestResult {
    // Given
    let state = TempState::new("runtime")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let auth_clock = clock(1_000);
    let control = CurrentUserRuntime::open_at(
        state.path(),
        Arc::clone(&auth_clock) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            ma2a_runtime::web::PasswordAction::Set,
            Zeroizing::new("web-runtime-passphrase-9!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let paths = IpcPaths::new(state.path())?;
    let api_server = LocalApiServer::bind(paths.clone(), runtime.handle(), control.clone())?;
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(api_server.serve(cancellation.child_token()));
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(paths),
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));
    let cookie = format!("ma2a_session={}", session.bearer());

    // When
    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .body(Body::empty())?,
        )
        .await?;
    let snapshot = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    let stale_events = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events?since=0")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;

    // Then
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(snapshot.into_body(), 65_536).await?)?;
    let revision = snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("snapshot revision missing")?;
    assert_eq!(stale_events.status(), StatusCode::OK);
    let events = to_bytes(stale_events.into_body(), 16_384).await?;
    assert!(
        events
            .windows(22)
            .any(|window| window == b"event: resync-required")
    );

    let mut streams = Vec::new();
    for _ in 0..8 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/events?since={revision}"))
                    .header("host", "127.0.0.1:43210")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response);
    }
    let limited = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?since={revision}"))
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    drop(streams.pop());
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/events?since={revision}"))
                        .header("host", "127.0.0.1:43210")
                        .header("cookie", &cookie)
                        .body(Body::empty())?,
                )
                .await?;
            if response.status() == StatusCode::OK {
                return TestResult::Ok(response);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    streams.push(replacement);

    control.send(Command::session_revoke_all()?).await?;
    let revoked = streams.pop().ok_or("active event stream missing")?;
    let mut revoked_body = revoked.into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), revoked_body.next())
        .await?
        .ok_or("revoked event stream closed without recovery event")??;
    assert!(
        frame
            .windows(22)
            .any(|window| window == b"event: resync-required")
    );
    drop(streams);

    let expiring_session = control
        .web_auth()
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let expiring_cookie = format!("ma2a_session={}", expiring_session.bearer());
    let expiring_snapshot = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &expiring_cookie)
                .body(Body::empty())?,
        )
        .await?;
    let expiring_snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(expiring_snapshot.into_body(), 65_536).await?)?;
    let expiring_revision = expiring_snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("expiring snapshot revision missing")?;
    let expiring_stream = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?since={expiring_revision}"))
                .header("host", "127.0.0.1:43210")
                .header("cookie", &expiring_cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(expiring_stream.status(), StatusCode::OK);
    let mut expiring_body = expiring_stream.into_body().into_data_stream();

    auth_clock.set(1_000 + WebAuthConfig::ABSOLUTE_TIMEOUT_MS);
    let expired = tokio::time::timeout(std::time::Duration::from_secs(2), expiring_body.next())
        .await?
        .ok_or("expired event stream closed without recovery event")??;
    assert_eq!(
        expired.as_ref(),
        format!("event: resync-required\ndata: {{\"revision\":{expiring_revision}}}\n\n")
            .as_bytes()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), expiring_body.next())
            .await?
            .is_none()
    );

    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}
