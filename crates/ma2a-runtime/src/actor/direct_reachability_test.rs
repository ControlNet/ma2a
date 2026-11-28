use std::sync::Arc;

use ma2a_core::AddressEndpointDataV1;
use ma2a_net::{EndpointBindOptions, IrohRelayObservation, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::StoreConfig;
use serde_json::json;
use tokio::sync::mpsc;

use super::relay_lifecycle_test::TempState;
use crate::{
    RuntimeClock,
    actor::{Actor, RuntimeHandle},
    error::RuntimeError,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(1_700_000_000_000)
    }
}

struct ActorFixture {
    _state_dir: TempState,
    actor: Actor,
    handle: RuntimeHandle,
    store_task: tokio::task::JoinHandle<()>,
}

async fn build_actor(
    label: &str,
    direct_reachable: bool,
    empty_endpoint_data: bool,
) -> Result<ActorFixture, Box<dyn std::error::Error + Send + Sync>> {
    let state_dir = TempState::new(label)?;
    let config = StoreConfig::new(&state_dir.0);
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let relay_map = store
        .load_relay_map(identity.endpoint_id, 1_700_000_000_000)
        .await?;
    let lookup = SpaceAddressLookup::default();
    let (enrollment_sender, enrollment_calls) = mpsc::channel(1);
    let (control_sender, control_calls) = mpsc::channel(1);
    let (echo_sender, echo_calls) = mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enrollment_sender, identity.bind_port)
            .with_control(control_sender, false)
            .with_echo(echo_sender)
            .with_relay_map(relay_map.clone()),
    )
    .await?;
    let endpoint_data = if empty_endpoint_data {
        AddressEndpointDataV1::from_parts(Vec::new(), None)?
    } else {
        endpoint.endpoint_data()?
    };
    let status = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data,
        boot_id: [0x52; 16],
        revision: store.revision().await?,
        memberships: std::collections::BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    };
    let echo_metrics = endpoint.echo_metrics();
    let (actor, handle, _cancellation) = Actor::new(
        status,
        endpoint,
        store,
        enrollment_calls,
        control_calls,
        echo_calls,
        echo_metrics,
        lookup,
        Arc::new(FixedClock),
        None,
    );
    Ok(ActorFixture {
        _state_dir: state_dir,
        actor,
        handle,
        store_task,
    })
}

fn json_u64(value: &serde_json::Value, field: &str) -> Result<u64, &'static str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing integer field")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_reachable_flip_without_endpoint_or_relay_change_advances_view_sequence()
-> TestResult {
    let mut fixture = build_actor("direct-view-seq", false, false).await?;
    let endpoint_id = fixture.actor.state.endpoint_id;
    let endpoint_data = fixture.actor.state.endpoint_data.clone();
    assert!(
        fixture
            .actor
            .state
            .endpoint_addr
            .ip_addrs()
            .next()
            .is_some(),
        "bound endpoint must expose at least one direct IP address"
    );
    let (obs_tx, obs_rx) = mpsc::channel(4);
    fixture.actor.relay_observations = obs_rx;
    let actor_task = tokio::spawn(fixture.actor.run());
    // Wait for the startup control round so it does not advance view_sequence during the check.
    fixture.handle.sync_control().await?;

    let before_stamp = fixture.handle.snapshot_stamp().await?.to_value();
    assert_eq!(
        fixture
            .handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/reachability/direct"),
        Some(&json!(false))
    );
    let before_revision = json_u64(&before_stamp, "revision")?;
    let before_view = json_u64(&before_stamp, "view_sequence")?;

    obs_tx
        .send(IrohRelayObservation::new(
            endpoint_id,
            endpoint_data,
            Vec::new(),
        )?)
        .await?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while fixture
        .handle
        .snapshot()
        .await?
        .to_value()
        .pointer("/reachability/direct")
        != Some(&json!(true))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "direct_reachable did not update to true"
        );
        tokio::task::yield_now().await;
    }

    let after_stamp = fixture.handle.snapshot_stamp().await?.to_value();
    assert_eq!(json_u64(&after_stamp, "revision")?, before_revision);
    let after_view = json_u64(&after_stamp, "view_sequence")?;
    assert!(
        after_view > before_view,
        "flipping direct_reachable without revision bump did not advance view_sequence ({before_view} -> {after_view})"
    );
    assert_eq!(
        fixture.handle.snapshot().await?.view_sequence(),
        after_view,
        "snapshot and snapshot_stamp must report identical view_sequence"
    );

    drop(obs_tx);
    fixture.handle.shutdown().await?;
    actor_task.await??;
    fixture.store_task.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_reachable_true_to_false_flip_without_revision_bump_advances_view_sequence()
-> TestResult {
    let mut fixture = build_actor("direct-view-seq-off", true, true).await?;
    let before_view = fixture.actor.view_sequence;
    let before_rev = fixture.actor.state.revision;
    let empty_data = fixture.actor.state.endpoint_data.clone();
    fixture.actor.maintenance.pending_observation = Some(IrohRelayObservation::new(
        fixture.actor.state.endpoint_id,
        empty_data,
        Vec::new(),
    )?);
    fixture.actor.reconcile_pending_iroh_observation().await?;
    assert!(!fixture.actor.state.direct_reachable);
    assert_eq!(fixture.actor.state.revision, before_rev);
    assert!(fixture.actor.view_sequence > before_view);

    let Actor {
        endpoint,
        store,
        relay_observer,
        ..
    } = fixture.actor;
    endpoint.shutdown().await?;
    relay_observer.await?;
    store.stop().await?;
    fixture.store_task.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_start_initializes_direct_reachable_from_endpoint_addr() -> TestResult {
    let state_dir = TempState::new("initial-direct")?;
    let runtime = crate::Runtime::start(StoreConfig::new(&state_dir.0)).await?;
    let handle = runtime.handle();
    let status = handle.status().await?;
    let expected = status.endpoint_addr().ip_addrs().next().is_some();
    let snapshot = handle.snapshot().await?.to_value();
    assert_eq!(
        snapshot.pointer("/reachability/direct"),
        Some(&json!(expected))
    );
    runtime.shutdown().await?;
    Ok(())
}
