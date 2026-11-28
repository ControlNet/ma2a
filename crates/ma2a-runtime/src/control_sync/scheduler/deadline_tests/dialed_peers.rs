use std::{future::pending, sync::Arc};

use ma2a_net::{
    CONTROL_DIAL_CONCURRENCY, CONTROL_ROUND_DEADLINE, EndpointBindOptions, RuntimeEndpoint,
    SpaceAddressLookup,
};
use ma2a_store::StoreConfig;
use tokio::sync::mpsc;

use super::{
    fixture::{FixedClock, TempState, TestResult},
    store_queue::seed_queued_peers,
};
use crate::{
    control_sync::{ControlRoundRequest, ControlRoundRunner, ControlRoundScope},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

#[tokio::test(start_paused = true)]
async fn undialed_prepared_peers_are_excluded_from_dialed_peers_at_deadline() -> TestResult {
    let dir = TempState::new("undialed-peers")?;
    let backend = StoreBackend::open(&StoreConfig::new(&dir.0))?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let local_id = identity.endpoint_id;
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(mpsc::channel(1).0, None),
    )
    .await?;

    seed_queued_peers(&dir.0, local_id)?;
    let started_peers = Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
    let runner = ControlRoundRunner::new(
        store.clone(),
        endpoint.control_client(),
        Arc::new(FixedClock(10_000)),
    );
    let round_task = tokio::spawn(runner.run_with_dialer(
        ControlRoundRequest {
            local_endpoint_id: local_id,
            rotation: 0,
            now_ms: 10_000,
            scope: ControlRoundScope::all(),
        },
        {
            let started_peers = Arc::clone(&started_peers);
            move |peer, _| {
                started_peers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(peer);
                async move { pending::<Result<Vec<u8>, std::io::Error>>().await }
            }
        },
    ));

    while started_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
        < CONTROL_DIAL_CONCURRENCY
    {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(CONTROL_ROUND_DEADLINE).await;
    let outcome = round_task
        .await??
        .ok_or("expired round must return an incomplete outcome")?;
    let started = started_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(started.len(), CONTROL_DIAL_CONCURRENCY);
    assert_eq!(
        outcome.dialed_peers, started,
        "dialed_peers must record only peers whose dial started"
    );

    store.stop().await?;
    store_task.await?;
    drop(endpoint);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn unpolled_submitted_dial_is_excluded_from_dialed_peers_at_deadline() -> TestResult {
    let peer = ma2a_net::EndpointSecret::parse(&[0x45; 32])?.endpoint_id();
    let mut queue = super::super::DialQueue::new([crate::control_sync::PreparedControlPeer {
        peer,
        request: vec![1],
        spaces: Vec::new(),
    }]);
    let dialed_peers = Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
    let mut dials = tokio::task::JoinSet::new();
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(1));
    tokio::pin!(deadline);
    tokio::time::advance(std::time::Duration::from_millis(1)).await;

    super::super::spawn_control_dials((&mut queue, &dialed_peers), &mut dials, {
        let started = Arc::clone(&started);
        move |_, _| {
            let started = Arc::clone(&started);
            async move {
                started.store(true, std::sync::atomic::Ordering::SeqCst);
                pending::<Result<Vec<u8>, std::io::Error>>().await
            }
        }
    });
    let result = super::super::run_before_deadline(&mut deadline, dials.join_next()).await;
    drop(dials);

    assert!(result.is_err());
    assert!(!started.load(std::sync::atomic::Ordering::SeqCst));
    let dialed_peers = dialed_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        dialed_peers.is_empty(),
        "peer submitted to JoinSet but dropped before first poll was recorded as dialed: {dialed_peers:?}"
    );
    Ok(())
}
