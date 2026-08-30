use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::{ProtocolError, RequestId};
use ma2a_store::StoreConfig;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    Runtime,
    api::{self, CommandResult, RuntimeStatusView},
    current_user::CurrentUserRuntime,
    ipc::{IpcError, IpcPaths, LocalApiClient, ServerExit},
    web::{SystemClock, WebAuthConfig},
};

use super::{LocalApiServer, ReplayEntry};

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
    let server = LocalApiServer::bind(paths.clone(), runtime.handle(), control)?;
    let request_id = RequestId::try_from(&[1_u8; 16][..])?;
    {
        let mut replay = server.replay.lock().await;
        replay.insert(
            request_id,
            ReplayEntry {
                fingerprint: [0_u8; 32],
                result: CommandResult::shutting_down(),
                revision: 0,
            },
        );
    }
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
async fn control_sync_commands_report_unsynchronized_without_spaces() -> TestResult {
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
        status_response.pointer("/result/payload/synchronized"),
        Some(&serde_json::Value::Bool(false))
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
