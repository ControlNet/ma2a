use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_store::StoreConfig;
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

use crate::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        PasswordAction, SystemClock, WebAssets, WebAuthConfig, WebRuntimeDependencies,
        WebServerConfig, build_runtime_router,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type TestRouter = (
    super::tests::TestState,
    Runtime,
    CancellationToken,
    tokio::task::JoinHandle<Result<crate::ipc::ServerExit, crate::ipc::IpcError>>,
    IpcPaths,
    Router,
    String,
);

fn json_u64(value: &serde_json::Value, key: &str) -> TestResult<u64> {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("missing {key}").into())
}

fn round_count(value: &serde_json::Value) -> TestResult<usize> {
    value
        .get("control_rounds")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| "missing control_rounds".into())
}

async fn fetch_snapshot(router: &Router, cookie: &str) -> TestResult<(String, serde_json::Value)> {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43212")
                .header("cookie", cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let boot_id = response
        .headers()
        .get("x-ma2a-runtime-boot-id")
        .ok_or("missing boot id header")?
        .to_str()?
        .to_owned();
    let payload = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    Ok((boot_id, payload))
}

async fn start_periodic_router() -> TestResult<TestRouter> {
    let path = std::env::temp_dir().join(format!(
        "ma2a-sse-idle-periodic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let state = super::tests::TestState(path.clone());
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
            Zeroizing::new("test-only-sse-periodic-8!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("test-only-sse-periodic-8!".to_owned()))
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
    .layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        12_348,
    ))));
    Ok((state, runtime, cancel, task, paths, router, cookie))
}

async fn poll_paused_sse<S>(sse_body: &mut S) -> TestResult<String>
where
    S: tokio_stream::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..20 {
            tokio::select! {
                biased;
                maybe = sse_body.next() => {
                    if let Some(Ok(chunk)) = maybe {
                        let text = String::from_utf8_lossy(&chunk);
                        if text.contains("snapshot_invalidated") || text.contains("resync-required") {
                            return Ok(text.into_owned());
                        }
                    }
                }
                () = tokio::task::yield_now() => {}
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Err("timed out waiting for SSE invalidation after idle periodic round".into())
}

#[tokio::test(start_paused = true)]
async fn idle_periodic_control_round_invalidates_sse() -> TestResult {
    let (_state, runtime, cancel, task, paths, router, cookie) = start_periodic_router().await?;

    runtime.handle().sync_control().await?;
    let endpoint_id = runtime.handle().status().await?.endpoint_id();
    let (boot_id, before) = fetch_snapshot(&router, &cookie).await?;
    let before_revision = json_u64(&before, "revision")?;
    let before_view_sequence = json_u64(&before, "view_sequence")?;
    let before_rounds = round_count(&before)?;
    let periodic_before = runtime
        .handle()
        .control_schedules()
        .iter()
        .filter(|t| matches!(t, crate::control_sync::ControlRoundTrigger::Periodic))
        .count();

    let sse_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={before_revision}&view_sequence={before_view_sequence}&boot_id={boot_id}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let mut sse_body = sse_resp.into_body().into_data_stream();

    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while runtime
        .handle()
        .control_schedules()
        .iter()
        .filter(|t| matches!(t, crate::control_sync::ControlRoundTrigger::Periodic))
        .count()
        <= periodic_before
    {
        assert!(
            std::time::Instant::now() < deadline,
            "periodic round did not schedule"
        );
        tokio::task::yield_now().await;
    }
    while round_count(&fetch_snapshot(&router, &cookie).await?.1)? <= before_rounds {
        assert!(
            std::time::Instant::now() < deadline,
            "periodic round did not append control_rounds"
        );
        tokio::task::yield_now().await;
    }

    let event = poll_paused_sse(&mut sse_body).await?;
    assert!(event.contains("snapshot_invalidated") || event.contains("resync-required"));
    drop(sse_body);

    let (_, after) = fetch_snapshot(&router, &cookie).await?;
    let after_revision = json_u64(&after, "revision")?;
    let after_view_sequence = json_u64(&after, "view_sequence")?;
    assert_eq!(after_revision, before_revision);
    assert!(after_view_sequence > before_view_sequence);
    assert_eq!(round_count(&after)?, before_rounds + 1);

    let stale_view = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={after_revision}&view_sequence={before_view_sequence}&boot_id={boot_id}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(stale_view.status(), StatusCode::OK);
    let stale_body = to_bytes(stale_view.into_body(), 16_384).await?;
    assert!(
        stale_body
            .windows(22)
            .any(|window| window == b"event: resync-required")
    );

    cancel.cancel();
    task.await??;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    Ok(())
}
