use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_mutation_fails_closed_after_runtime_restart() -> TestResult {
    // Given
    let state = TempState::new()?;
    let paths = IpcPaths::new(&state.0)?;
    let command = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"03030303030303030303030303030303","name":"interrupted"}"#,
    )?;
    let request_id = command.request_id().ok_or("request id missing")?;
    let fingerprint = api::command_fingerprint(&command)?;
    let first_runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    first_runtime
        .handle()
        .reserve_mutation_replay(request_id, fingerprint)
        .await?;
    first_runtime.shutdown().await?;
    let second_runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), second_runtime.handle(), control, None)
            .await?,
    );
    let client = LocalApiClient::new(paths);
    client.probe().await?;
    let conflict = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"03030303030303030303030303030303","name":"changed"}"#,
    )?;

    // When
    let retry: serde_json::Value = serde_json::from_slice(&client.call(&command).await?)?;
    let changed: serde_json::Value = serde_json::from_slice(&client.call(&conflict).await?)?;

    // Then
    assert_eq!(
        retry.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name())
    );
    assert_eq!(
        changed.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name())
    );
    assert!(
        second_runtime
            .handle()
            .snapshot()
            .await?
            .spaces()
            .is_empty()
    );
    assert_eq!(server.cancel().await?, ServerExit::Cancelled);
    second_runtime.shutdown().await?;
    Ok(())
}
