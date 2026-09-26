use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_invitation_file_failure_cannot_mint_again_after_restart() -> TestResult {
    let state = TempState::new()?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let space = runtime
        .handle()
        .create_owned_space("replay".to_owned())
        .await?;
    let command = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "space_invite",
        "request_id": "12121212121212121212121212121212",
        "space_id": api::encode_hex(space.as_bytes()), "ttl_ms": 60000,
        // An existing directory deterministically fails create_new after commit.
        "output_path": state.0.to_str().ok_or("invalid path")?,
    }))?)?;
    let mut first_error = None;
    let mut runtime = Some(runtime);
    for _ in 0..2 {
        let current = match runtime.take() {
            Some(runtime) => runtime,
            None => Runtime::start(config.clone()).await?,
        };
        let control = CurrentUserRuntime::open_at(
            &state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&state.0)?;
        let server = LiveServer::spawn(
            LocalApiServer::bind_for_launch(paths.clone(), current.handle(), control, None).await?,
        );
        let client = LocalApiClient::new(paths.clone());
        for _ in 0..2 {
            let response = client.call(&command).await?;
            let value: serde_json::Value = serde_json::from_slice(&response)?;
            assert!(value.get("error").is_some());
            if let Some(first) = &first_error {
                assert_eq!(&response, first);
            } else {
                first_error = Some(response);
            }
            let mut changed: serde_json::Value =
                serde_json::from_slice(&api::encode_command(&command)?)?;
            *changed.get_mut("ttl_ms").ok_or("missing ttl")? = serde_json::json!(30000);
            let conflict = api::decode_command(&serde_json::to_vec(&changed)?)?;
            let conflict: serde_json::Value =
                serde_json::from_slice(&client.call(&conflict).await?)?;
            assert_eq!(
                conflict.get("error").and_then(serde_json::Value::as_str),
                Some("conflict")
            );
            let connection = rusqlite::Connection::open(config.database_path())?;
            let count: u64 =
                connection.query_row("SELECT COUNT(*) FROM invitations", [], |row| row.get(0))?;
            assert_eq!(
                count, 1,
                "a failed completion must not reopen invitation issuance"
            );
        }
        server.cancel().await?;
        paths.remove_stale_endpoint()?;
        current.shutdown().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_completion_failure_preserves_pending_after_durable_invitation() -> TestResult {
    let state = TempState::new()?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let space = runtime
        .handle()
        .create_owned_space("pending".to_owned())
        .await?;
    let command = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "space_invite",
        "request_id": "13131313131313131313131313131313",
        "space_id": api::encode_hex(space.as_bytes()), "ttl_ms": 60000,
        "output_path": state.0.to_str().ok_or("invalid path")?,
    }))?)?;
    let connection = rusqlite::Connection::open(config.database_path())?;
    connection.execute_batch("CREATE TRIGGER fail_replay_completion BEFORE UPDATE ON local_mutation_replay BEGIN SELECT RAISE(ABORT, 'test completion failure'); END;")?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?,
    );
    let client = LocalApiClient::new(paths);
    assert!(client.call(&command).await.is_err());
    connection.execute_batch("DROP TRIGGER fail_replay_completion")?;
    let retry: serde_json::Value = serde_json::from_slice(&client.call(&command).await?)?;
    assert_eq!(
        retry.get("error").and_then(serde_json::Value::as_str),
        Some("unavailable")
    );
    let count: u64 =
        connection.query_row("SELECT COUNT(*) FROM invitations", [], |row| row.get(0))?;
    assert_eq!(count, 1);
    assert!(matches!(
        ma2a_store::Repository::open(&config)?
            .mutation_replay(command.request_id().ok_or("request id")?)?,
        Some(ma2a_store::MutationReplayState::Pending(_))
    ));
    server.cancel().await?;
    runtime.shutdown().await?;
    Ok(())
}
