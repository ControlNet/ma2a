use std::{sync::Arc, time::Duration};

use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{Repository, StoreConfig};
use tokio::sync::mpsc;

use super::{FixedClock, NOW_MS, TempState, TestResult};
use crate::{
    actor::Actor,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finish_completes_while_relay_observer_waits_on_a_full_channel() -> TestResult {
    // Given
    let state_dir = TempState::new()?;
    let config = StoreConfig::new(state_dir.path());
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let repository = Repository::open(&config)?;
    let relay_map = store
        .load_relay_map(identity.endpoint_id, u64::try_from(NOW_MS)?)
        .await?;
    let lookup = SpaceAddressLookup::default();
    let (enrollment_sender, enrollment_calls) = mpsc::channel(1);
    let (control_sender, control_calls) = mpsc::channel(1);
    let (echo_sender, echo_calls) = mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enrollment_sender, identity.bind_port)
            .with_control(control_sender, true)
            .with_echo(echo_sender)
            .with_relay_map(relay_map.clone()),
    )
    .await?;
    let status = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x53; 16],
        revision: repository.revision()?,
        memberships: repository.memberships_for(identity.endpoint_id)?,
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    };
    let echo_metrics = endpoint.echo_metrics();
    let (mut actor, _handle, _cancellation) = Actor::new(
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
    // The observer has filled the channel and waits to send one more snapshot,
    // which no endpoint closure can interrupt.
    let observation = tokio::time::timeout(Duration::from_secs(2), actor.relay_observations.recv())
        .await?
        .ok_or("initial relay observation channel closed")?;
    let (sender, observations) = mpsc::channel(1);
    sender.send(observation.clone()).await?;
    actor.relay_observations = observations;
    let blocked = tokio::spawn(async move {
        let _closed = sender.send(observation).await;
    });
    let previous = std::mem::replace(&mut actor.relay_observer, blocked);
    tokio::task::yield_now().await;

    // When
    let finished = tokio::time::timeout(Duration::from_secs(5), actor.finish(true)).await;

    // Then
    assert!(
        finished.is_ok(),
        "shutdown waited on the blocked relay observer"
    );
    finished??;
    previous.await?;
    store_task.await?;
    Ok(())
}
