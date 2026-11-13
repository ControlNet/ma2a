use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_reclaims_the_capacity_of_abandoned_reservations() -> TestResult {
    // Given: a Runtime that stopped with every replay slot held by an unsettled request.
    let state = TempState::new_named("replay-full")?;
    let config = StoreConfig::new(&state.0);
    let first = Runtime::start(config.clone()).await?;
    let limit = u16::try_from(ma2a_store::LOCAL_MUTATION_REPLAY_MAX_ENTRIES)?;
    for value in 0..limit {
        let request = ma2a_core::RequestId::try_from(value.to_be_bytes().repeat(8).as_slice())?;
        first
            .handle()
            .reserve_mutation_replay(request, [1; 32])
            .await?;
    }
    first.shutdown().await?;
    let second = Runtime::start(config).await?;
    let paths = IpcPaths::new(&state.0)?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), second.handle(), control, None).await?,
    );
    let command = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","name":"after-restart"}"#,
    )?;

    // When
    let reply: serde_json::Value =
        serde_json::from_slice(&LocalApiClient::new(paths).call(&command).await?)?;

    // Then
    assert!(reply.get("result").is_some(), "{reply}");
    let abandoned = ma2a_core::RequestId::try_from([0_u8; 16].as_slice())?;
    assert_eq!(
        second.handle().mutation_replay(abandoned).await?,
        Some(ma2a_store::MutationReplayState::Pending([1; 32]))
    );
    assert_eq!(server.cancel().await?, ServerExit::Cancelled);
    second.shutdown().await?;
    Ok(())
}

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
