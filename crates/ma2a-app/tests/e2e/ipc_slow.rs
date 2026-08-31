use std::sync::Arc;

use ma2a_runtime::{
    Runtime,
    api::decode_command,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{SystemClock, WebAuthConfig},
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use super::harness::{TempState, TestResult, emit};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_partial_ipc_frame_is_bounded_while_unrelated_client_progresses() -> TestResult {
    let state = TempState::new("slow-ipc")?;
    let config = state.config();
    let database_path = config.database_path();
    let state_dir = database_path.parent().ok_or("state path missing")?;
    let runtime = Runtime::start(config.clone()).await?;
    let control = CurrentUserRuntime::open_at(
        state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(state_dir)?;
    let server = LocalApiServer::bind(paths.clone(), runtime.handle(), control)?;
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(server.serve(cancellation.child_token()));

    let mut slow = tokio::net::UnixStream::connect(paths.socket_path()).await?;
    slow.write_all(&64_u32.to_be_bytes()).await?;
    slow.write_all(&41_u64.to_be_bytes()).await?;
    slow.write_all(b"{").await?;
    let command = decode_command(br#"{"version":1,"operation":"status"}"#)?;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        LocalApiClient::new(paths).call(&command),
    )
    .await??;
    assert!(
        response
            .windows(15)
            .any(|value| value == b"\"type\":\"status\"")
    );
    let mut closed = [0_u8; 1];
    let read =
        tokio::time::timeout(std::time::Duration::from_secs(3), slow.read(&mut closed)).await??;
    assert_eq!(read, 0);
    emit(&serde_json::json!({
        "scenario": "slow-ipc-frame",
        "endpoint_ids": {"runtime": runtime.handle().status().await?.endpoint_id().to_public_key()?.to_string()},
        "partial_payload_bytes": 1,
        "declared_payload_bytes": 64,
        "unrelated_client_progressed": true,
        "bounded_rejection": true
    }));
    cancellation.cancel();
    task.await??;
    runtime.shutdown().await?;
    Ok(())
}
