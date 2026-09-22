use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig, build_runtime_router,
    },
};
use n0_future::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

use super::harness::{TempState, TestResult, emit};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_queue_overflow_resyncs_and_dropped_receiver_releases_permit() -> TestResult {
    let state = TempState::new("sse-overflow")?;
    let config = state.config();
    let database_path = config.database_path();
    let state_dir = database_path.parent().ok_or("state path missing")?;
    let runtime = Runtime::start(config).await?;
    let clock = Arc::new(ma2a_runtime::web::SystemClock::default());
    let control = CurrentUserRuntime::open_at(state_dir, clock, WebAuthConfig::default()).await?;
    control
        .web_auth()
        .change_password(
            ma2a_runtime::web::PasswordAction::Set,
            Zeroizing::new("sse-overflow-passphrase-9!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("sse-overflow-passphrase-9!".to_owned()))
        .await?;
    let paths = IpcPaths::new(state_dir)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control.clone(), None)
            .await?;
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(server.serve(cancellation.child_token()));
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(paths),
        ),
        WebServerConfig::default(),
        43_211,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_346))));
    let cookie = format!("ma2a_session={}", session.bearer());
    let revision = current_revision(&router, &cookie).await?;
    let mut streams = Vec::new();
    for _ in 0..8 {
        streams.push(open_stream(&router, revision, &cookie).await?);
    }
    assert_eq!(
        open_stream(&router, revision, &cookie).await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(streams.pop());
    let replacement = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let response = open_stream(&router, revision, &cookie).await?;
            if response.status() == StatusCode::OK {
                return Result::<_, Box<dyn std::error::Error + Send + Sync>>::Ok(response);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    streams.push(replacement);
    drop(streams);

    let overflow = open_stream(&router, revision, &cookie).await?;
    assert_eq!(overflow.status(), StatusCode::OK);
    let mut body = overflow.into_body().into_data_stream();
    let mut cadence = tokio::time::interval(Duration::from_millis(1_100));
    cadence.tick().await;
    for marker in 1_u8..=9 {
        runtime
            .handle()
            .observe_memberships(vec![ma2a_core::SpaceId::derive(&[marker; 32])])
            .await?;
        cadence.tick().await;
    }
    let mut bytes = Vec::new();
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(2), body.next()).await? {
        bytes.extend_from_slice(&frame?);
    }
    assert!(
        bytes
            .windows(22)
            .any(|window| window == b"event: resync-required"),
        "stream carried no resync: {}",
        String::from_utf8_lossy(&bytes)
    );
    emit(&serde_json::json!({
        "scenario": "sse-overflow-receiver-cleanup",
        "endpoint_ids": {"runtime": runtime.handle().status().await?.endpoint_id().to_public_key()?.to_string()},
        "queued_revisions": 9,
        "resync_required": true,
        "dropped_receiver_permit_reused": true
    }));
    cancellation.cancel();
    task.await??;
    runtime.shutdown().await?;
    Ok(())
}

/// Reads the revision the Runtime would accept as an event-stream baseline now.
async fn current_revision(
    router: &axum::Router,
    cookie: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let snapshot = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43211")
                .header("cookie", cookie)
                .body(Body::empty())?,
        )
        .await?;
    let snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(snapshot.into_body(), 65_536).await?)?;
    snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "snapshot revision missing".into())
}

async fn open_stream(
    router: &axum::Router,
    revision: u64,
    cookie: &str,
) -> Result<axum::response::Response, Box<dyn std::error::Error + Send + Sync>> {
    Ok(router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?since={revision}"))
                .header("host", "127.0.0.1:43211")
                .header("cookie", cookie)
                .body(Body::empty())?,
        )
        .await?)
}
