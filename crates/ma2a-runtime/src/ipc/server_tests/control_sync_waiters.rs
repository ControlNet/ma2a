//! Merged targeted rounds distinguish dialed and undialed waiters even when zero commits occur.

use std::{sync::Arc, time::Duration};

use iroh::SecretKey;
use ma2a_core::{
    AddressRecordScope, AddressRecordValidity, EndpointId, MemberCapabilities, RequestId,
    SpaceAddressRecordV1, SpaceAuthorizationView, SpaceManifestMembership, SpaceMemberV1,
    default_member_label,
};
use ma2a_net::{
    ControlRejection, EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, StoreConfig,
    ValidatedAddressRecord,
};
use tokio::sync::mpsc;

use super::*;
use crate::{
    RuntimeClock, RuntimeError,
    control_sync::{ControlRoundScope, ControlRoundTrigger},
};

#[derive(Debug)]
struct FixedClock(i64);

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0)
    }
}

async fn setup_rejecting_peer(
    config: &StoreConfig,
    local: &Runtime,
    local_id: EndpointId,
) -> TestResult<(EndpointId, RuntimeEndpoint, tokio::task::JoinHandle<()>)> {
    let peer_seed = [0x4b; 32];
    let peer_key = SecretKey::from_bytes(&peer_seed);
    let peer_secret = EndpointSecret::parse(&peer_seed)?;
    let peer_id = peer_secret.endpoint_id();
    let (enroll_tx, _enroll_rx) = mpsc::channel(1);
    let (control_tx, mut control_rx) = mpsc::channel(8);
    let peer_endpoint = RuntimeEndpoint::bind_with_lookup(
        peer_secret,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(enroll_tx, None).with_control(control_tx, true),
    )
    .await?;
    let peer_data = peer_endpoint.endpoint_data()?;
    let space_id = local
        .handle()
        .create_owned_space("merged-waiters".to_owned())
        .await?;
    let caps = MemberCapabilities::new(true, true);
    let mut members = vec![
        SpaceMemberV1::new(local_id, default_member_label(local_id), caps)?,
        SpaceMemberV1::new(peer_id, "rejecting-peer".to_owned(), caps)?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    local
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space_id,
            10_000,
            SpaceManifestMembership::new(members, Vec::new()),
        ))
        .await?;
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(1, 10_000, 610_000)?,
        peer_data,
    )
    .sign(&peer_key)?;
    let mut repository = Repository::open(config)?;
    let shared = repository
        .control_spaces_for(local_id)?
        .into_iter()
        .find(|space| space.chain().space_id() == space_id)
        .ok_or("shared space missing")?;
    let auth = SpaceAuthorizationView::from_chain(shared.chain());
    let validated = ValidatedAddressRecord::parse(
        signed.canonical_bytes(),
        AddressRecordValidation::new(AddressRecordTarget::new(space_id, peer_id), &auth, 10_000),
    )?;
    repository.advance_validated_address(&validated)?;
    let peer_task = tokio::spawn(async move {
        while let Some(call) = control_rx.recv().await {
            let Some(call) = call.authorize() else {
                continue;
            };
            if let Ok((_request, responder)) = call.request().await {
                responder.respond(Err(ControlRejection::Invalid));
            }
        }
    });
    Ok((peer_id, peer_endpoint, peer_task))
}

struct TriggerCall {
    peer: EndpointId,
    request_id: RequestId,
    task: tokio::task::JoinHandle<Result<Vec<u8>, IpcError>>,
}

fn spawn_sync_trigger(
    paths: IpcPaths,
    request_hex: &str,
    peer_hex: &str,
) -> TestResult<TriggerCall> {
    let command = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"{request_hex}","peer_endpoint_id":"{peer_hex}"}}"#
        )
        .as_bytes(),
    )?;
    let peer = command.control_sync_peer().ok_or("peer missing")?;
    let request_id = command.request_id().ok_or("request id missing")?;
    let task = tokio::spawn(async move { LocalApiClient::new(paths).call(&command).await });
    Ok(TriggerCall {
        peer,
        request_id,
        task,
    })
}

async fn space_create_call(
    client: &LocalApiClient,
    request_hex: &str,
    name: &str,
) -> TestResult<serde_json::Value> {
    let command = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"space_create","request_id":"{request_hex}","name":"{name}"}}"#
        )
        .as_bytes(),
    )?;
    Ok(serde_json::from_slice(&client.call(&command).await?)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merged_targeted_round_failure_releases_undialed_peer_request_id() -> TestResult {
    let state = TempState::new_named("merged-waiters")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock(10_000))).await?;
    let local_id = runtime.handle().status().await?.endpoint_id();
    let (dialed_peer, peer_endpoint, peer_task) =
        setup_rejecting_peer(&config, &runtime, local_id).await?;
    let dialed_hex = api::encode_hex(dialed_peer.as_bytes());
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

    let (entered, release) = runtime.handle().arm_control_round_gate();
    let blocker = tokio::spawn({
        let handle = runtime.handle();
        async move { handle.sync_control().await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered).await??;

    let dialed_req = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    let undialed_req = "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2";
    let dialed = spawn_sync_trigger(paths.clone(), dialed_req, &dialed_hex)?;
    let undialed = spawn_sync_trigger(paths.clone(), undialed_req, PEER_ENDPOINT_ID)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let schedules = runtime.handle().control_schedules();
            if schedules.contains(&ControlRoundTrigger::Explicit(ControlRoundScope::peer(
                dialed.peer,
            ))) && schedules.contains(&ControlRoundTrigger::Explicit(ControlRoundScope::peer(
                undialed.peer,
            ))) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;

    let _released = release.send(());
    let _blocker = tokio::time::timeout(Duration::from_secs(5), blocker).await?;
    let dialed_resp: serde_json::Value = serde_json::from_slice(
        &tokio::time::timeout(Duration::from_secs(5), dialed.task).await???,
    )?;
    let undialed_resp: serde_json::Value = serde_json::from_slice(
        &tokio::time::timeout(Duration::from_secs(5), undialed.task).await???,
    )?;

    let replay_dialed = runtime.handle().mutation_replay(dialed.request_id).await?;
    let replay_undialed = runtime
        .handle()
        .mutation_replay(undialed.request_id)
        .await?;
    let client = LocalApiClient::new(paths.clone());
    let reuse_undialed = space_create_call(&client, undialed_req, "reused-after-undialed").await?;
    let reuse_dialed = space_create_call(&client, dialed_req, "reused-after-dialed").await?;

    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    drop(peer_endpoint);
    peer_task.abort();
    runtime.shutdown().await?;

    assert_eq!(
        dialed_resp.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name())
    );
    assert_eq!(
        undialed_resp
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name())
    );
    assert_eq!(
        undialed_resp.get("remediation"),
        Some(&serde_json::Value::Null)
    );
    assert!(replay_dialed.is_some(), "dialed peer must fence dialed_req");
    assert_eq!(
        replay_undialed, None,
        "undialed peer must release undialed_req"
    );
    assert_eq!(
        reuse_undialed.get("error"),
        None,
        "released undialed_req must be accepted on reuse: {reuse_undialed}"
    );
    assert_eq!(
        reuse_dialed
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "fenced dialed_req must conflict on different payload: {reuse_dialed}"
    );
    Ok(())
}
