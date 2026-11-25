//! Remote network mutations (`echo_call` and `control_sync_trigger`) release the replay lock
//! once their request ID is reserved, and a trigger for a peer with no shared Space has no effect.

use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use tokio::time::timeout;

use super::*;
use crate::{EnrollmentAttempt, EnrollmentCreation};

const PROMPT: Duration = Duration::from_secs(2);

fn error_name(reply: &[u8]) -> TestResult<Option<String>> {
    let value: serde_json::Value = serde_json::from_slice(reply)?;
    Ok(value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

async fn member_with_offline_owner(name: &str) -> TestResult<(TempState, Runtime, String)> {
    let owner_state = TempState::new_named(&format!("{name}-owner"))?;
    let member_state = TempState::new_named(&format!("{name}-member"))?;
    let owner = Runtime::start(StoreConfig::new(&owner_state.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_state.0)).await?;
    let owner_hex = api::encode_hex(owner.handle().status().await?.endpoint_id().as_bytes());
    let space = owner.handle().create_owned_space(name.to_owned()).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x91; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;
    Ok((member_state, member, owner_hex))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echo_and_control_sync_trigger_do_not_block_unrelated_mutations() -> TestResult {
    // Given: a member served over IPC whose peer is offline.
    let (member_state, member, owner_hex) = member_with_offline_owner("replay-lock").await?;
    let paths = IpcPaths::new(&member_state.0)?;
    let control = CurrentUserRuntime::open_at(
        &member_state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), member.handle(), control, None).await?,
    );
    let echo = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"echo_call","request_id":"92929292929292929292929292929292","target_endpoint_id":"{owner_hex}","payload":"ping"}}"#
        )
        .as_bytes(),
    )?;
    let trigger = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"93939393939393939393939393939393","peer_endpoint_id":"{owner_hex}"}}"#
        )
        .as_bytes(),
    )?;
    let create_a = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"94949494949494949494949494949494","name":"during-echo"}"#,
    )?;
    let create_b = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"95959595959595959595959595959595","name":"during-sync"}"#,
    )?;

    // When: echo_call waits on the offline peer while space_create runs on another connection.
    let client_a = LocalApiClient::new(paths.clone());
    let echoing = tokio::spawn(async move { client_a.call(&echo).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let during_echo = timeout(PROMPT, LocalApiClient::new(paths.clone()).call(&create_a)).await??;
    assert!(!echoing.is_finished(), "echo_call must still be waiting");
    assert_eq!(error_name(&during_echo)?, None);

    // And when: control_sync_trigger waits on the offline peer while another space_create runs.
    let client_b = LocalApiClient::new(paths.clone());
    let syncing = tokio::spawn(async move { client_b.call(&trigger).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let during_sync = timeout(PROMPT, LocalApiClient::new(paths).call(&create_b)).await??;
    assert!(
        !syncing.is_finished(),
        "control_sync_trigger must still be waiting"
    );
    assert_eq!(error_name(&during_sync)?, None);

    // Then: stopping the Runtime ends both pending calls cleanly.
    timeout(Duration::from_secs(5), member.shutdown()).await??;
    let _echo_end = timeout(Duration::from_secs(5), echoing).await?;
    let _sync_end = timeout(Duration::from_secs(5), syncing).await?;
    drop(server);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_sync_trigger_for_an_unrelated_peer_releases_its_request_id() -> TestResult {
    // Given: a Runtime that shares no Space with PEER_ENDPOINT_ID.
    let state = TempState::new_named("sync-no-space")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
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
    let trigger = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"96969696969696969696969696969696","peer_endpoint_id":"{PEER_ENDPOINT_ID}"}}"#
        )
        .as_bytes(),
    )?;
    let request_id = trigger.request_id().ok_or("request id missing")?;

    // When
    let answer: serde_json::Value =
        serde_json::from_slice(&LocalApiClient::new(paths.clone()).call(&trigger).await?)?;
    let replay = runtime.handle().mutation_replay(request_id).await?;

    // Then: nothing was dialed, so the request ID is released without a side-effects warning.
    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    assert_eq!(
        answer.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name())
    );
    assert_eq!(answer.get("remediation"), Some(&serde_json::Value::Null));
    assert_eq!(replay, None);
    Ok(())
}
