use super::*;
use ma2a_core::{MemberCapabilities, SpaceManifestMembership, SpaceMemberV1};
use ma2a_store::{OwnedSpaceUpdate, Repository};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handshake_never_waits_for_a_mutation_in_progress() -> TestResult {
    // Given: a revoke paused while it holds the replay lock.
    let state = TempState::new()?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("paused".to_owned()).await?;
    let initial = Repository::open(&config)?
        .load_space_chain(space)?
        .ok_or("missing Space")?;
    let peer = SpaceMemberV1::new(
        iroh::SecretKey::generate().public().into(),
        "peer".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let mut members = initial.members().to_vec();
    members.push(peer.clone());
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    handle
        .advance_owned_space(OwnedSpaceUpdate::new(
            space,
            1000,
            SpaceManifestMembership::new(members, vec![]),
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
        "version": 1, "operation": "space_revoke", "request_id": "15".repeat(16),
        "space_id": api::encode_hex(space.as_bytes()),
        "peer_endpoint_id": api::encode_hex(peer.endpoint_id().as_bytes()),
    }))?)?;
    let revoking = {
        let paths = paths.clone();
        tokio::spawn(async move { LocalApiClient::new(paths).call(&command).await })
    };
    paused.await?;

    // When: a client checks the daemon, as every upgrade and command does first.
    let probed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        LocalApiClient::new(paths).probe(),
    )
    .await;

    // Then
    assert!(matches!(probed, Ok(Ok(()))), "{probed:?}");
    resume.send(()).map_err(|()| "request ended")?;
    revoking.await??;
    server.cancel().await?;
    runtime.shutdown().await?;
    Ok(())
}
