use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::ProtocolError;
use ma2a_store::{MutationReplayRecord, MutationReplayRequest, StoreConfig};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    Runtime,
    api::{self, ApiResponse, CommandResult, RuntimeStatusView},
    current_user::CurrentUserRuntime,
    ipc::{IpcError, IpcPaths, LocalApiClient, ServerExit},
    web::{SystemClock, WebAuthConfig},
};

use super::LocalApiServer;

#[path = "server_tests/pending_replay.rs"]
mod pending_replay;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PEER_ENDPOINT_ID: &str = "5866666666666666666666666666666666666666666666666666666666666666";

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-shutdown-server-{}-{serial}",
            std::process::id()
        ));
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

struct LiveServer {
    cancellation: CancellationToken,
    task: Option<JoinHandle<Result<ServerExit, IpcError>>>,
}

impl LiveServer {
    fn spawn(server: LocalApiServer) -> Self {
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(server.serve(cancellation.child_token()));
        Self {
            cancellation,
            task: Some(task),
        }
    }

    async fn cancel(mut self) -> TestResult<ServerExit> {
        self.cancellation.cancel();
        let task = self
            .task
            .take()
            .ok_or_else(|| std::io::Error::other("live server task missing"))?;
        Ok(task.await??)
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_shutdown_request_keeps_live_server_available() -> TestResult {
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
    let handle = runtime.handle();
    let server = LocalApiServer::bind(paths.clone(), handle.clone(), control)?;
    let seeded = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"01010101010101010101010101010101","name":"seed"}"#,
    )?;
    let request_id = seeded.request_id().ok_or("seed request id missing")?;
    let response = api::encode_response(&ApiResponse::new(
        Some(request_id),
        0,
        CommandResult::shutting_down(),
    ))?;
    let fingerprint = api::command_fingerprint(&seeded)?;
    handle
        .reserve_mutation_replay(request_id, fingerprint)
        .await?;
    handle
        .record_mutation_replay(MutationReplayRecord::new(
            MutationReplayRequest::new(request_id, fingerprint),
            0,
            response,
        )?)
        .await?;
    let live_server = LiveServer::spawn(server);
    let client = LocalApiClient::new(paths);
    client.probe().await?;
    let conflicting = api::decode_command(
        br#"{"version":1,"operation":"graceful_shutdown","request_id":"01010101010101010101010101010101"}"#,
    )?;

    // When
    let rejected = client.call(&conflicting).await?;

    // Then
    let rejected_value: serde_json::Value = serde_json::from_slice(&rejected)?;
    assert_eq!(
        rejected_value
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name())
    );
    let status = api::decode_command(br#"{"version":1,"operation":"status"}"#)?;
    let status_response = client.call(&status).await?;
    let status_value: serde_json::Value = serde_json::from_slice(&status_response)?;
    let expected_status_type = CommandResult::status(RuntimeStatusView::new(0, true, false));
    assert_eq!(
        status_value
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some(expected_status_type.result_type())
    );
    assert_eq!(live_server.cancel().await?, ServerExit::Cancelled);
    let shutdown = runtime.shutdown().await?;
    assert_eq!(shutdown.joined_tasks(), 2);
    assert!(shutdown.endpoint_closed());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn successful_mutation_replays_and_conflicts_after_runtime_restart() -> TestResult {
    // Given
    let state = TempState::new()?;
    let paths = IpcPaths::new(&state.0)?;
    let command = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"02020202020202020202020202020202","name":"durable"}"#,
    )?;
    let first_runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let first_control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let first_server = LiveServer::spawn(LocalApiServer::bind(
        paths.clone(),
        first_runtime.handle(),
        first_control,
    )?);
    let first_client = LocalApiClient::new(paths.clone());
    first_client.probe().await?;
    let committed = first_client.call(&command).await?;
    assert_eq!(first_server.cancel().await?, ServerExit::Cancelled);
    paths.remove_stale_endpoint()?;
    first_runtime.shutdown().await?;
    let second_runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let second_control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let second_server = LiveServer::spawn(LocalApiServer::bind(
        paths.clone(),
        second_runtime.handle(),
        second_control,
    )?);
    let second_client = LocalApiClient::new(paths);
    second_client.probe().await?;
    let conflict = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"02020202020202020202020202020202","name":"conflict"}"#,
    )?;

    // When
    let replayed = second_client.call(&command).await?;
    let rejected = second_client.call(&conflict).await?;

    // Then
    assert_eq!(replayed, committed);
    let rejected_value: serde_json::Value = serde_json::from_slice(&rejected)?;
    assert_eq!(
        rejected_value
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name())
    );
    let snapshot = second_runtime.handle().snapshot().await?;
    assert_eq!(snapshot.spaces().len(), 1);
    assert_eq!(second_server.cancel().await?, ServerExit::Cancelled);
    second_runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_sync_commands_report_no_peers_without_spaces() -> TestResult {
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
    let live_server = LiveServer::spawn(server);
    let client = LocalApiClient::new(paths);
    client.probe().await?;
    let status = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_status","peer_endpoint_id":"{PEER_ENDPOINT_ID}"}}"#
        )
        .as_bytes(),
    )?;
    let trigger = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"01010101010101010101010101010101","peer_endpoint_id":"{PEER_ENDPOINT_ID}"}}"#
        )
        .as_bytes(),
    )?;

    // When
    let status_response: serde_json::Value = serde_json::from_slice(&client.call(&status).await?)?;
    let trigger_response: serde_json::Value =
        serde_json::from_slice(&client.call(&trigger).await?)?;

    // Then
    assert_eq!(
        status_response
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("control_sync_status")
    );
    assert_eq!(
        status_response.pointer("/result/payload"),
        Some(&serde_json::json!({ "peer_endpoint_ids": [] }))
    );
    assert_eq!(
        trigger_response
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name())
    );
    assert_eq!(live_server.cancel().await?, ServerExit::Cancelled);
    runtime.shutdown().await?;
    Ok(())
}
