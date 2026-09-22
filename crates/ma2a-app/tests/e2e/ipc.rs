use std::sync::Arc;

use ma2a_runtime::{
    Runtime,
    api::decode_command,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{SystemClock, WebAuthConfig},
};
use tokio_util::sync::CancellationToken;

use super::harness::{TempState, TestResult};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_private_ipc_clients_share_one_runtime_and_teardown() -> TestResult {
    // Given
    let state = TempState::new("ipc")?;
    let config = state.config();
    let runtime = Runtime::start(config.clone()).await?;
    let endpoint_id = runtime.handle().status().await?.endpoint_id();
    let control = CurrentUserRuntime::open_at(
        config
            .database_path()
            .parent()
            .ok_or("state path missing")?,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(
        config
            .database_path()
            .parent()
            .ok_or("state path missing")?,
    )?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(server.serve(cancellation.child_token()));
    let command = decode_command(br#"{"version":1,"operation":"status"}"#)?;

    // When
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let client = LocalApiClient::new(paths.clone());
        let command = command.clone();
        calls.spawn(async move { client.call(&command).await });
    }
    while let Some(result) = calls.join_next().await {
        let response = result??;
        assert!(
            response
                .windows(15)
                .any(|value| value == b"\"type\":\"status\"")
        );
    }

    // Then
    assert_eq!(runtime.handle().status().await?.endpoint_id(), endpoint_id);
    cancellation.cancel();
    task.await??;
    assert_eq!(runtime.shutdown().await?.joined_tasks(), 2);
    Ok(())
}
