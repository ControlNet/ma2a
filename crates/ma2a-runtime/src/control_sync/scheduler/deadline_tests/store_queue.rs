use std::{
    future::pending,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, EndpointId,
    MemberCapabilities, SpaceAddressRecordV1, SpaceAuthorizationView, SpaceId,
    SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    CONTROL_DIAL_CONCURRENCY, CONTROL_ROUND_DEADLINE, EndpointBindOptions, RuntimeEndpoint,
    SpaceAddressLookup,
};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, SpaceCreation,
    StoreConfig, ValidatedAddressRecord,
};
use tokio::sync::{mpsc, oneshot};

use super::fixture::{FixedClock, TempState, TestResult, setup_fixture};
use super::wait_for_generation;
use crate::{
    control_sync::{
        ControlRoundRequest, ControlRoundRunner, ControlRoundScope, ControlRoundTrigger,
    },
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

pub(super) fn seed_queued_peers(
    dir: &Path,
    local_id: EndpointId,
) -> Result<(EndpointId, SpaceId), Box<dyn std::error::Error + Send + Sync>> {
    let mut repo = Repository::open(&StoreConfig::new(dir))?;
    let caps = MemberCapabilities::new(true, true);
    let local_m = SpaceMemberV1::new(local_id, "local".to_owned(), caps)?;
    let mut first = None;
    for index in 0..=CONTROL_DIAL_CONCURRENCY {
        let seed = 0x70_u8.wrapping_add(u8::try_from(index)?);
        let peer_key = SecretKey::from_bytes(&[seed; 32]);
        let peer_id = EndpointId::from(peer_key.public());
        let peer_m = SpaceMemberV1::new(peer_id, format!("peer-{index}"), caps)?;
        let space_id = repo
            .create_owned_space(&SpaceCreation::new(
                10_000,
                local_m.clone(),
                SpacePolicyV1::phase_one_default(),
            ))?
            .space_id();
        let mut members = vec![local_m.clone(), peer_m];
        members.sort_by_key(SpaceMemberV1::endpoint_id);
        let advanced = repo.advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            10_000,
            SpaceManifestMembership::new(members, Vec::new()),
        ))?;
        let signed = SpaceAddressRecordV1::new(
            AddressRecordScope::new(space_id, peer_id),
            AddressRecordValidity::new(1, 10_000, 610_000)?,
            AddressEndpointDataV1::new(Vec::new())?,
        )
        .sign(&peer_key)?;
        let auth = SpaceAuthorizationView::from_chain(advanced.chain());
        let validated = ValidatedAddressRecord::parse(
            signed.canonical_bytes(),
            AddressRecordValidation::new(
                AddressRecordTarget::new(space_id, peer_id),
                &auth,
                10_000,
            ),
        )?;
        repo.advance_validated_address(&validated)?;
        if first.is_none() {
            first = Some((peer_id, space_id));
        }
    }
    Ok(first.ok_or("first peer missing")?)
}

#[tokio::test(start_paused = true)]
async fn last_response_held_in_store_queue_across_deadline_fails_and_keeps_commits() -> TestResult {
    let mut f = setup_fixture("deadline-store-queue", 0x61).await?;
    let (reply, waiter) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply))
        .ok_or("no global round")?;
    let (owner_id, gen2_response, empty_response) = (
        f.owner_id,
        f.gen2_response.clone(),
        f.empty_response.clone(),
    );
    let (peer_b_gate_tx, peer_b_gate_rx) = oneshot::channel::<()>();
    let peer_b_gate_rx = Arc::new(tokio::sync::Mutex::new(Some(peer_b_gate_rx)));

    let runner = ControlRoundRunner::new(
        f.actor.store.clone(),
        f.actor.endpoint.control_client(),
        Arc::clone(&f.actor.clock),
    );
    let request = ControlRoundRequest {
        local_endpoint_id: f.actor.state.endpoint_id,
        rotation: global.rotation(),
        now_ms: 10_000,
        scope: global.scope().clone(),
    };
    let round_task = tokio::spawn(runner.run_with_dialer(request, move |peer, _| {
        let (gen2, empty) = (gen2_response.clone(), empty_response.clone());
        let gate = Arc::clone(&peer_b_gate_rx);
        async move {
            if peer == owner_id {
                Ok::<Vec<u8>, std::io::Error>(gen2)
            } else {
                let pending_gate = gate.lock().await.take();
                if let Some(rx) = pending_gate {
                    let _ = rx.await;
                }
                Ok::<Vec<u8>, std::io::Error>(empty)
            }
        }
    }));

    wait_for_generation(&f.actor.store, f.space_id, 2).await?;
    let mut permits = Vec::new();
    while let Ok(permit) = f.actor.store.sender.try_reserve() {
        permits.push(permit);
    }
    assert_eq!(permits.len(), STORE_CAPACITY);
    let _ = peer_b_gate_tx.send(());
    tokio::task::yield_now().await;

    tokio::time::advance(CONTROL_ROUND_DEADLINE).await;
    tokio::task::yield_now().await;
    drop(permits);
    let outcome = round_task.await?;

    let committed_rev = f.actor.store.revision().await?;
    f.actor.finish_control_round(Ok((global, outcome))).await?;
    assert!(
        waiter.await?.is_err(),
        "round whose last response waited past deadline in Store queue must fail"
    );
    let last_round = f.actor.control_round_history.back().ok_or("no history")?;
    assert_eq!(last_round.outcome, "failed");
    assert!(f.actor.state.revision >= committed_rev);
    assert!(!f.actor.synchronized_control_peers.contains(&f.peer_b_id));
    f.actor.reconcile_control_completion().await?;
    let triggers = f
        .actor
        .control_schedule_events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(triggers.contains(&ControlRoundTrigger::ManifestAdvanced));

    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn no_new_dial_starts_after_deadline_expires() -> TestResult {
    let dir = TempState::new("nodial")?;
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

    let (first_peer, first_space) = seed_queued_peers(&dir.0, local_id)?;
    let empty_response = ma2a_core::ControlResponseV1::new(vec![ma2a_core::ControlPageV1::new(
        first_space,
        false,
        Vec::new(),
    )?])?
    .encode()?;

    let dial_count = Arc::new(AtomicUsize::new(0));
    let (first_gate_tx, first_gate_rx) = oneshot::channel::<()>();
    let first_gate_rx = Arc::new(tokio::sync::Mutex::new(Some(first_gate_rx)));
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
            let dial_count = Arc::clone(&dial_count);
            let first_gate_rx = Arc::clone(&first_gate_rx);
            move |peer, _| {
                dial_count.fetch_add(1, Ordering::SeqCst);
                let first_gate_rx = Arc::clone(&first_gate_rx);
                let empty_response = empty_response.clone();
                async move {
                    if peer == first_peer {
                        let pending_gate = first_gate_rx.lock().await.take();
                        if let Some(rx) = pending_gate {
                            let _ = rx.await;
                        }
                        Ok::<Vec<u8>, std::io::Error>(empty_response)
                    } else {
                        pending::<Result<Vec<u8>, std::io::Error>>().await
                    }
                }
            }
        },
    ));

    while dial_count.load(Ordering::SeqCst) < CONTROL_DIAL_CONCURRENCY {
        tokio::task::yield_now().await;
    }
    let mut permits = Vec::new();
    while let Ok(permit) = store.sender.try_reserve() {
        permits.push(permit);
    }
    let _ = first_gate_tx.send(());
    tokio::task::yield_now().await;

    tokio::time::advance(CONTROL_ROUND_DEADLINE).await;
    tokio::task::yield_now().await;
    drop(permits);
    let _outcome = round_task.await?;

    assert_eq!(
        dial_count.load(Ordering::SeqCst),
        CONTROL_DIAL_CONCURRENCY,
        "round scheduled an additional dial after the aggregate deadline expired"
    );

    store.stop().await?;
    store_task.await?;
    drop(endpoint);
    Ok(())
}
