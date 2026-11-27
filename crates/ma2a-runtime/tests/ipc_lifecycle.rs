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
    ipc::{IpcPaths, LocalApiClient, LocalApiServer, ServerExit},
    web::{PasswordAction, SystemClock, WebAuthConfig},
};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

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
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test state directory")?;
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
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
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
    assert_eq!(report.joined_tasks(), 3);
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
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn password_set_updates_daemon_handshake_and_authoritative_revision() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let initial_revision = runtime.handle().status().await?.revision();
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));
    let client = LocalApiClient::new(paths);

    // When
    let set_response = client
        .call(&Command::ui_password_set(Zeroizing::new(
            "daemon-handshake-passphrase-9!".to_owned(),
        ))?)
        .await?;
    let handshake = decode_command(br#"{"version":1,"operation":"handshake"}"#)?;
    let handshake_response = client.call(&handshake).await?;

    // Then
    let set_response: serde_json::Value = serde_json::from_slice(&set_response)?;
    let handshake_response: serde_json::Value = serde_json::from_slice(&handshake_response)?;
    let mutation_revision = set_response
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("password response revision missing")?;
    assert!(mutation_revision > initial_revision);
    assert_eq!(
        handshake_response
            .pointer("/result/payload/password_set")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        handshake_response
            .get("revision")
            .and_then(serde_json::Value::as_u64),
        Some(mutation_revision)
    );
    cancellation.cancel();
    server_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

async fn call_raw_command(
    client: &LocalApiClient,
    payload: &[u8],
) -> TestResult<serde_json::Value> {
    let command = decode_command(payload)?;
    Ok(serde_json::from_slice(&client.call(&command).await?)?)
}

fn json_u64(value: &serde_json::Value, pointer: &str) -> TestResult<u64> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("missing u64 at {pointer}").into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_control_sync_keeps_handshake_password_and_receipt_revisions_durable() -> TestResult
{
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
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));
    let client = LocalApiClient::new(paths);

    // Run two explicit control rounds before any local mutation.
    runtime.handle().sync_control().await?;
    runtime.handle().sync_control().await?;

    // When: ui_password_set followed by handshake.
    let set_command =
        Command::ui_password_set(Zeroizing::new("durable-revision-sync-pass-9!".to_owned()))?;
    let set_response: serde_json::Value =
        serde_json::from_slice(&client.call(&set_command).await?)?;
    let password_revision = json_u64(&set_response, "/revision")?;
    let handshake_response =
        call_raw_command(&client, br#"{"version":1,"operation":"handshake"}"#).await?;

    // Then: handshake and password-set revisions agree after explicit control syncs.
    assert_eq!(
        json_u64(&handshake_response, "/revision")?,
        password_revision
    );
    assert_eq!(
        json_u64(&handshake_response, "/result/payload/revision")?,
        password_revision
    );

    // When: snapshot_fetch followed by space_create, status, and space_details_fetch.
    let snapshot_before =
        call_raw_command(&client, br#"{"version":1,"operation":"snapshot_fetch"}"#).await?;
    let snapshot_revision = json_u64(&snapshot_before, "/result/payload/revision")?;
    assert_eq!(json_u64(&snapshot_before, "/revision")?, snapshot_revision);
    assert!(json_u64(&snapshot_before, "/result/payload/view_sequence")? >= 2);

    let space_create_response = call_raw_command(
        &client,
        br#"{"version":1,"operation":"space_create","request_id":"a00102030405060708090a0b0c0d0e0f","name":"durable-check"}"#,
    )
    .await?;
    let created_revision = json_u64(&space_create_response, "/revision")?;
    let space_id = space_create_response
        .pointer("/result/payload/space_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("space_id missing")?;

    let status_response =
        call_raw_command(&client, br#"{"version":1,"operation":"status"}"#).await?;
    let status_revision = json_u64(&status_response, "/result/payload/revision")?;

    let details_payload =
        format!(r#"{{"version":1,"operation":"space_details_fetch","space_id":"{space_id}"}}"#);
    let details_response = call_raw_command(&client, details_payload.as_bytes()).await?;
    let details_revision = json_u64(&details_response, "/result/payload/revision")?;

    // Then: no receipt or query returns a lower revision than the prior snapshot in the same boot.
    assert!(
        created_revision > snapshot_revision,
        "space_create receipt ({created_revision}) regressed below earlier snapshot ({snapshot_revision})"
    );
    assert!(
        status_revision >= created_revision,
        "status ({status_revision}) regressed below space_create ({created_revision})"
    );
    assert!(
        details_revision >= created_revision,
        "space_details ({details_revision}) regressed below space_create ({created_revision})"
    );
    cancellation.cancel();
    server_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_fetch_returns_the_authoritative_runtime_projection() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let status = runtime.handle().status().await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            PasswordAction::Set,
            Zeroizing::new("snapshot-revision-passphrase-9!".to_owned()),
        )
        .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));
    let command = decode_command(br#"{"version":1,"operation":"snapshot_fetch"}"#)?;

    // When
    let response = LocalApiClient::new(paths).call(&command).await?;

    // Then
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        response
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("snapshot")
    );
    let response_revision = response
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("response revision missing")?;
    let snapshot_revision = response
        .pointer("/result/payload/revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("snapshot revision missing")?;
    assert!(snapshot_revision > status.revision());
    assert_eq!(response_revision, snapshot_revision);
    assert_eq!(
        response
            .get("runtime_boot_id")
            .and_then(serde_json::Value::as_str)
            .map(str::len),
        Some(32)
    );
    assert_eq!(
        response
            .pointer("/result/payload/endpoint/online")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        response.pointer("/result/payload/spaces"),
        Some(&serde_json::json!([]))
    );
    assert_eq!(
        response
            .pointer("/result/payload/ui_auth/password_set")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    cancellation.cancel();
    server_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_stops_accepting_once_its_runtime_stops() -> TestResult {
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
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let server_task = tokio::spawn(server.serve(cancellation.child_token()));

    // When
    let _report = runtime.shutdown().await?;
    let exit = server_task.await??;

    // Then
    assert!(matches!(exit, ServerExit::RuntimeStopped));
    assert!(!LocalApiClient::new(paths).is_live().await);
    Ok(())
}
