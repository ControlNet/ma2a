use std::{sync::Arc, time::Duration};

use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{Repository, StoreConfig};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{FixedClock, NOW_MS, TempState, TestError, TestResult};
use crate::{
    actor::{Actor, Command, RuntimeHandle},
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

/// An actor over a live store and a bound endpoint, as the Runtime starts one.
async fn started() -> Result<(Actor, RuntimeHandle, JoinHandle<()>, TempState), TestError> {
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
    Ok((actor, handle, store_task, state_dir))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finish_completes_while_relay_observer_waits_on_a_full_channel() -> TestResult {
    // Given
    let (mut actor, _handle, store_task, _state_dir) = started().await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_that_won_queue_room_before_finish_is_refused_not_stranded() -> TestResult {
    // Given: a caller that has room in the command queue but has not handed its command
    // over yet, as `Sender::send` does between its two steps.
    let (actor, handle, store_task, _state_dir) = started().await?;
    let permit = handle.commands.reserve().await?;

    // When: the actor finishes, and only then the command lands.
    let mut finishing = tokio::spawn(actor.finish(true));
    let _finished = tokio::time::timeout(Duration::from_secs(2), &mut finishing).await;
    let (reply, response) = oneshot::channel();
    permit.send(Command::Status(reply));

    // Then: the caller learns the actor is gone instead of waiting forever.
    let answered = tokio::time::timeout(Duration::from_secs(5), response).await;
    assert!(
        matches!(answered, Ok(Err(_))),
        "a command that reached a finished actor must be refused, not left unanswered"
    );
    tokio::time::timeout(Duration::from_secs(5), finishing).await???;
    store_task.await?;
    Ok(())
}
