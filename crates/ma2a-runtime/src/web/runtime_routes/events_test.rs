use std::sync::Arc;

use ma2a_store::StoreConfig;
use zeroize::Zeroizing;

use super::*;
use crate::web::{PasswordAction, SystemClock, WebAuthConfig, WebAuthService};

#[tokio::test]
async fn revocation_during_snapshot_read_prevents_notification()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = std::env::temp_dir().join(format!("ma2a-sse-revoke-poll-{}", std::process::id()));
    let state = TestState(path);
    let auth = WebAuthService::open(
        StoreConfig::new(&state.0),
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(
        PasswordAction::Set,
        Zeroizing::new("test-only-sse-revocation-8!".to_owned()),
    )
    .await?;
    let session = auth
        .login(Zeroizing::new("test-only-sse-revocation-8!".to_owned()))
        .await?;

    // This deterministic projection future represents revocation committing while
    // the producer awaits IPC. Authentication itself uses the real SQLite Store.
    let result = authenticated_stamp(&auth, session.bearer(), async {
        auth.revoke_all_sessions()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(SnapshotStamp {
            revision: 7,
            boot_id: "00112233445566778899aabbccddeeff".to_owned(),
        })
    })
    .await;
    assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    Ok(())
}

pub(super) struct TestState(std::path::PathBuf);

impl Drop for TestState {
    fn drop(&mut self) {
        let _result = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn equal_revision_restart_with_stale_boot_id_emits_resync_required()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tokio_stream::StreamExt as _;
    use tower::ServiceExt as _;

    let (_state, runtime, cancel, task, paths, router, cookie) = start_test_router().await?;

    let snapshot_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(snapshot_resp.status(), StatusCode::OK);
    let current_boot = snapshot_resp
        .headers()
        .get("x-ma2a-runtime-boot-id")
        .ok_or("missing boot id header")?
        .to_str()?
        .to_owned();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(snapshot_resp.into_body(), 65_536).await?)?;
    let revision = snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing revision")?;
    let prior_boot = if current_boot == "00".repeat(16) {
        "ff".repeat(16)
    } else {
        "00".repeat(16)
    };

    // Subscribing at the same revision with a different boot_id must immediately resync.
    let stale_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={revision}&boot_id={prior_boot}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(stale_resp.status(), StatusCode::OK);
    let body = tokio::time::timeout(
        Duration::from_millis(500),
        to_bytes(stale_resp.into_body(), 16_384),
    )
    .await
    .map_err(|_| "stale boot_id subscription did not close with resync-required")??;
    assert!(
        body.windows(22)
            .any(|window| window == b"event: resync-required"),
        "expected resync-required after equal-revision restart, got: {}",
        String::from_utf8_lossy(&body)
    );

    // Matching boot_id and revision starts a quiet stream without immediate resync.
    let live_resp = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={revision}&boot_id={current_boot}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(live_resp.status(), StatusCode::OK);
    let mut live_body = live_resp.into_body().into_data_stream();
    let initial = tokio::time::timeout(Duration::from_millis(200), live_body.next()).await;
    assert!(
        initial.is_err(),
        "matching boot_id and revision must stay quiet, got {initial:?}"
    );
    drop(live_body);

    cancel.cancel();
    task.await??;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    Ok(())
}

pub(super) type TestRouter = (
    TestState,
    crate::Runtime,
    tokio_util::sync::CancellationToken,
    tokio::task::JoinHandle<Result<crate::ipc::ServerExit, crate::ipc::IpcError>>,
    crate::ipc::IpcPaths,
    axum::Router,
    String,
);

pub(super) async fn start_test_router()
-> Result<TestRouter, Box<dyn std::error::Error + Send + Sync>> {
    use std::net::SocketAddr;

    use axum::extract::connect_info::MockConnectInfo;
    use tokio_util::sync::CancellationToken;

    use crate::{
        Runtime,
        current_user::CurrentUserRuntime,
        ipc::{IpcPaths, LocalApiClient, LocalApiServer},
        web::{
            WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig, build_runtime_router,
        },
    };

    let path = std::env::temp_dir().join(format!(
        "ma2a-sse-boot-restart-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let state = TestState(path.clone());
    let runtime = Runtime::start(StoreConfig::new(&path)).await?;
    let control = CurrentUserRuntime::open_at(
        &path,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            PasswordAction::Set,
            Zeroizing::new("test-only-sse-boot-restart-8!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("test-only-sse-boot-restart-8!".to_owned()))
        .await?;
    let cookie = format!("ma2a_session={}", session.bearer());
    let paths = IpcPaths::new(&path)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control.clone(), None)
            .await?;
    let cancel = CancellationToken::new();
    let task = tokio::spawn(server.serve(cancel.child_token()));
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(paths.clone()),
        ),
        WebServerConfig::default(),
        43_212,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_347))));
    Ok((state, runtime, cancel, task, paths, router, cookie))
}
