use std::sync::Arc;

use ma2a_core::{MemberCapabilities, SpaceMemberV1, default_member_label};
use ma2a_net::{EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreConfig};
use tokio::sync::mpsc;

use super::{Actor, relay_lifecycle_test::TempState};
use crate::{
    control_sync::{ControlChanges, ControlRoundOutcome, ControlRoundScope},
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn actor_fixture(config: &StoreConfig) -> TestResult<(Actor, tokio::task::JoinHandle<()>)> {
    let backend = StoreBackend::open(config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let lookup = SpaceAddressLookup::default();
    let (enrollment, enrollment_calls) = mpsc::channel(1);
    let (control, control_calls) = mpsc::channel(1);
    let (echo, echo_calls) = mpsc::channel(1);
    let map = store.load_relay_map(identity.endpoint_id, 100).await?;
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enrollment, None)
            .with_control(control, false)
            .with_echo(echo)
            .with_relay_map(map.clone()),
    )
    .await?;
    let metrics = endpoint.echo_metrics();
    let state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x59; 16],
        revision: store.revision().await?,
        memberships: std::collections::BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let (actor, _, _) = Actor::new(
        state,
        endpoint,
        store,
        enrollment_calls,
        control_calls,
        echo_calls,
        metrics,
        lookup,
        Arc::new(crate::clock::SystemClock),
        None,
    );
    Ok((actor, task))
}

#[tokio::test]
async fn snapshot_between_round_commit_and_completion_still_advances_view_on_completion()
-> TestResult {
    let directory = TempState::new("control-mid-snapshot")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, store_task) = actor_fixture(&config).await?;
    let space_id = actor
        .create_owned_space("mid-snapshot".to_owned())
        .await?
        .space_id();
    while let Some(joined) = actor.control_rounds.join_next().await {
        actor.finish_control_round(joined).await?;
    }
    let peer = EndpointSecret::parse(&[0x44; 32])?.endpoint_id();
    let scheduled = actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no control round")?;

    let local_member = SpaceMemberV1::new(
        actor.state.endpoint_id,
        default_member_label(actor.state.endpoint_id),
        MemberCapabilities::new(true, true),
    )?;
    let peer_member =
        SpaceMemberV1::new(peer, "peer".to_owned(), MemberCapabilities::new(true, true))?;
    let mut members = vec![local_member, peer_member];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let committed_rev = Repository::open(&config)?
        .advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            20,
            ma2a_core::SpaceManifestMembership::new(members, Vec::new()),
        ))?
        .revision();

    let mid_snapshot = actor.snapshot().await?;
    assert_eq!(mid_snapshot.revision(), committed_rev);
    let mid_view = mid_snapshot.view_sequence();
    let mid_rounds = mid_snapshot
        .to_value()
        .get("control_rounds")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);

    actor
        .finish_control_round(Ok((
            scheduled,
            Ok(Some(ControlRoundOutcome {
                revision: Some(committed_rev),
                dialed_peers: [peer].into(),
                synchronized_peers: [peer].into(),
                changes: ControlChanges::default(),
                incomplete: false,
            })),
        )))
        .await?;

    let after_snapshot = actor.snapshot().await?;
    assert_eq!(after_snapshot.revision(), mid_snapshot.revision());
    assert_eq!(
        after_snapshot
            .to_value()
            .get("control_rounds")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(mid_rounds + 1)
    );
    assert!(
        after_snapshot.view_sequence() > mid_view,
        "mid-round snapshot masked control round completion (view_sequence {} -> {})",
        mid_view,
        after_snapshot.view_sequence()
    );
    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}
