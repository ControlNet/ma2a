use super::{
    CurrentUserRuntime, IpcPaths, LiveServer, LocalApiClient, LocalApiServer, Runtime, StoreConfig,
    SystemClock, TempState, TestResult, WebAuthConfig, api,
};
use ma2a_core::{MemberCapabilities, SpaceMemberV1, SpacePolicyV1};
use ma2a_store::{Repository, SpaceCreation};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legal_snapshot_above_single_frame_and_collection_limits_is_addressable() -> TestResult {
    let state = TempState::new_named("snapshot-stream")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let endpoint = runtime.handle().status().await?.endpoint_id();
    runtime.shutdown().await?;
    let mut repository = Repository::open(&config)?;
    let member = SpaceMemberV1::new(
        endpoint,
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let mut expected = Vec::new();
    for _ in 0..270 {
        let created = repository.create_owned_space(
            &SpaceCreation::new(10, member.clone(), SpacePolicyV1::phase_one_default())
                .with_name("\"\\".repeat(32)),
        )?;
        expected.push(api::encode_hex(created.space_id().as_bytes()));
    }
    drop(repository);
    expected.sort();
    let runtime = Runtime::start(config).await?;
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
    let bytes = client.call(&api::Command::snapshot_fetch()).await?;
    assert!(bytes.len() > api::MAX_LOCAL_RESPONSE_BYTES);
    let response: serde_json::Value = serde_json::from_slice(&bytes)?;
    let spaces = response
        .pointer("/result/payload/spaces")
        .and_then(serde_json::Value::as_array)
        .ok_or("no Spaces")?;
    let actual = spaces
        .iter()
        .map(|space| {
            space
                .get("space_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("no Space ID")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(actual, expected);
    assert_eq!(
        response.get("revision"),
        response.pointer("/result/payload/revision")
    );
    assert_eq!(
        response
            .get("runtime_boot_id")
            .and_then(serde_json::Value::as_str),
        Some(api::encode_hex(&runtime.handle().status().await?.boot_id()).as_str())
    );
    let list = api::decode_command(br#"{"version":1,"operation":"space_list"}"#)?;
    let listed: serde_json::Value = serde_json::from_slice(&client.call(&list).await?)?;
    assert_eq!(
        listed
            .pointer("/result/payload")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(270)
    );
    server.cancel().await?;
    runtime.shutdown().await?;
    Ok(())
}
