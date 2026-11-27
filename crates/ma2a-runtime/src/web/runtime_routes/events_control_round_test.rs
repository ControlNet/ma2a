use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_core::{EndpointId, InviteEntropy, RequestId, default_member_label};
use ma2a_store::StoreConfig;
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime,
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
    CurrentUserRuntime,
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

async fn start_router_with_control(label: &str) -> TestResult<TestRouter> {
    let path = std::env::temp_dir().join(format!(
        "ma2a-sse-{label}-{}-{}",
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
            Zeroizing::new("test-only-sse-commit-8!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("test-only-sse-commit-8!".to_owned()))
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
        12_349,
    ))));
    Ok((state, runtime, control, cancel, task, paths, router, cookie))
}

async fn enroll_and_sync(owner: &Runtime, member: &Runtime) -> TestResult {
    let space_id = owner
        .handle()
        .create_owned_space("commit-window".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    let member_id: EndpointId = member.handle().status().await?.endpoint_id();
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x91; 16].as_slice())?,
            default_member_label(member_id),
        ))
        .await?;
    member.handle().sync_control().await?;
    member.handle().sync_control().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_between_round_commit_and_completion_invalidates_on_round_finish() -> TestResult {
    let owner_dir = super::tests::TestState(std::env::temp_dir().join(format!(
        "ma2a-sse-commit-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    )));
    std::fs::create_dir(&owner_dir.0)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&owner_dir.0, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    ma2a_store::protect_private_directory(&owner_dir.0, "test owner state directory")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let (_member_state, member, control, cancel, task, paths, router, cookie) =
        start_router_with_control("commit-member").await?;
    enroll_and_sync(&owner, &member).await?;

    let before_rounds = round_count(&fetch_snapshot(&router, &cookie).await?.1)?;
    let (entered_rx, release_tx) = member.handle().arm_control_round_gate();
    let sync_handle = member.handle();
    let sync_task = tokio::spawn(async move { sync_handle.sync_control().await });
    entered_rx.await?;

    let _extra_session = control
        .web_auth()
        .login(Zeroizing::new("test-only-sse-commit-8!".to_owned()))
        .await?;
    let (boot_id, mid_round) = fetch_snapshot(&router, &cookie).await?;
    let mid_revision = json_u64(&mid_round, "revision")?;
    let mid_view_sequence = json_u64(&mid_round, "view_sequence")?;
    assert_eq!(round_count(&mid_round)?, before_rounds);

    let sse_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={mid_revision}&view_sequence={mid_view_sequence}&boot_id={boot_id}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let mut sse_body = sse_resp.into_body().into_data_stream();

    let _ = release_tx.send(());
    sync_task.await??;

    let event = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match sse_body.next().await {
                Some(Ok(chunk)) => {
                    let text = String::from_utf8_lossy(&chunk);
                    if text.contains("snapshot_invalidated") || text.contains("resync-required") {
                        return Ok::<String, String>(text.into_owned());
                    }
                }
                Some(Err(error)) => return Err(format!("sse error: {error}")),
                None => return Err("sse stream closed without invalidation".to_owned()),
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for SSE invalidation")??;
    assert!(event.contains("snapshot_invalidated") || event.contains("resync-required"));
    drop(sse_body);

    let (_, after_round) = fetch_snapshot(&router, &cookie).await?;
    assert!(json_u64(&after_round, "view_sequence")? > mid_view_sequence);
    assert_eq!(round_count(&after_round)?, before_rounds + 1);

    cancel.cancel();
    task.await??;
    paths.remove_stale_endpoint()?;
    member.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}
