//! Concurrent private IPC lifecycle integration coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_runtime::{
    Runtime,
    api::{Command, decode_command},
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{SystemClock, WebAuthConfig},
};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ma2a-ipc-{}-{serial}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_clients_share_one_runtime_endpoint_and_teardown_cleanly() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let endpoint_id = runtime.handle().status().await?.endpoint_id();
    let paths = IpcPaths::new(&state.0)?;
    let server = LocalApiServer::bind(paths.clone(), runtime.handle(), control)?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));
    let command = decode_command(br#"{"version":1,"operation":"status"}"#)?;

    // When
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let client = LocalApiClient::new(paths.clone());
        let command = command.clone();
        calls.spawn(async move { client.call(&command).await });
    }
    while let Some(result) = calls.join_next().await {
        let response = result??;
        assert!(
            response
                .windows(15)
                .any(|window| window == b"\"type\":\"status\"")
        );
    }
    let observed_id = runtime.handle().status().await?.endpoint_id();
    cancellation.cancel();
    server_task.await??;
    let report = runtime.shutdown().await?;

    // Then
    assert_eq!(observed_id, endpoint_id);
    assert_eq!(report.joined_tasks(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_session_control_executes_through_the_live_daemon() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server = LocalApiServer::bind(paths.clone(), runtime.handle(), control)?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));
    let command = Command::session_revoke_all()?;

    // When
    let response = LocalApiClient::new(paths).call(&command).await?;

    // Then
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        response
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("sessions_revoked")
    );
    cancellation.cancel();
    server_task.await??;
    runtime.shutdown().await?;
    Ok(())
}
