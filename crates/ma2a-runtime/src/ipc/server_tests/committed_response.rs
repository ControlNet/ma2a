use super::*;
use ma2a_core::{MemberCapabilities, SpaceManifestMembership, SpaceMemberV1};
use ma2a_store::{OwnedSpaceUpdate, Repository};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_response_and_revision_share_the_committed_membership() -> TestResult {
    let state = TempState::new()?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("coherent".to_owned()).await?;
    let initial = Repository::open(&config)?
        .load_space_chain(space)?
        .ok_or("missing Space")?;
    let peer = SpaceMemberV1::new(
        iroh::SecretKey::generate().public().into(),
        "peer".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let third = SpaceMemberV1::new(
        iroh::SecretKey::generate().public().into(),
        "third".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let mut members = initial.members().to_vec();
    members.push(peer.clone());
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    handle
        .advance_owned_space(OwnedSpaceUpdate::new(
            space,
            1000,
            SpaceManifestMembership::new(members.clone(), vec![]),
        ))
        .await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), handle.clone(), control, None).await?;
    let (arrived, paused) = tokio::sync::oneshot::channel();
    let (resume, gate) = tokio::sync::oneshot::channel();
    *server.mutation_hooks.before_revoke.lock().await = Some((arrived, gate));
    let server = LiveServer::spawn(server);
    let command = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "space_revoke", "request_id": "14141414141414141414141414141414",
        "space_id": api::encode_hex(space.as_bytes()), "peer_endpoint_id": api::encode_hex(peer.endpoint_id().as_bytes()),
    }))?)?;
    let request = tokio::spawn(async move { LocalApiClient::new(paths).call(&command).await });
    paused.await?;
    // Enrollment/other Actor work can advance the chain outside the IPC mutex.
    members.push(third);
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    handle
        .advance_owned_space(OwnedSpaceUpdate::new(
            space,
            2000,
            SpaceManifestMembership::new(members, vec![]),
        ))
        .await?;
    resume.send(()).map_err(|()| "request ended")?;
    let response: serde_json::Value = serde_json::from_slice(&request.await??)?;
    let repository = Repository::open(&config)?;
    let committed = repository
        .load_space_chain(space)?
        .ok_or("missing committed Space")?;
    assert_eq!(committed.members().len(), 2);
    assert_eq!(
        response
            .pointer("/result/payload/member_count")
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
    assert_eq!(
        response.get("revision").and_then(serde_json::Value::as_u64),
        Some(repository.revision()?)
    );
    server.cancel().await?;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn read_receipts_ignore_an_older_dispatch_status() -> TestResult {
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let older = handle.status().await?;
    let space = handle.create_owned_space("read-receipt".to_owned()).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server =
        LocalApiServer::bind_for_launch(IpcPaths::new(&state.0)?, handle.clone(), control, None)
            .await?;
    let (shutdown_sender, _receiver) = tokio::sync::mpsc::channel(1);
    let context = super::super::ConnectionContext {
        paths: server.paths.clone(),
        handle: handle.clone(),
        control: server.control.clone(),
        replay: Arc::clone(&server.replay),
        shutdown_sender,
        web: None,
        identity: Arc::clone(&server.identity),
        mutation_hooks: server.mutation_hooks.clone(),
    };
    let operations = [
        serde_json::json!({"version":1,"operation":"handshake"}),
        serde_json::json!({"version":1,"operation":"private_relay_status"}),
        serde_json::json!({"version":1,"operation":"public_relay_status"}),
        serde_json::json!({"version":1,"operation":"space_show", "space_id":api::encode_hex(space.as_bytes())}),
        serde_json::json!({"version":1,"operation":"control_sync_status", "peer_endpoint_id": PEER_ENDPOINT_ID}),
    ];
    for operation in operations {
        let command = api::decode_command(&serde_json::to_vec(&operation)?)?;
        let result = super::super::execute::execute(&command, &older, &context).await?;
        let revision = result.committed_revision().ok_or("read receipt missing")?;
        assert!(revision > older.revision());
        assert_eq!(revision, handle.snapshot().await?.revision());
    }
    drop(server);
    runtime.shutdown().await?;
    Ok(())
}
