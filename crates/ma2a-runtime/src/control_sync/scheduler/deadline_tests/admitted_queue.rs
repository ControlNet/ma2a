use std::{
    future::pending,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use iroh::SecretKey;
use ma2a_core::{
    EndpointId, MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::CONTROL_ROUND_DEADLINE;
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use tokio::sync::oneshot;

use super::{
    DialDropGuard,
    admitted_projection::{
        finish_round_without_blocking, hold_store, sign_address_seq2, wait_for_address_sequence,
    },
    fixture::{Fixture, TestResult, seed_address, setup_fixture},
};
use crate::control_sync::{
    ControlFailure, ControlRoundOutcome, ControlRoundRequest, ControlRoundRunner,
    ControlRoundScope, ControlRoundTrigger,
};

fn seed_hanging_peer(dir: &Path, local_id: EndpointId, seed: u8) -> TestResult<EndpointId> {
    let mut repo = Repository::open(&StoreConfig::new(dir))?;
    let caps = MemberCapabilities::new(true, true);
    let local_m = SpaceMemberV1::new(local_id, "local".to_owned(), caps)?;
    let peer_key = SecretKey::from_bytes(&[seed; 32]);
    let peer_id = EndpointId::from(peer_key.public());
    let peer_m = SpaceMemberV1::new(peer_id, "hanging-peer".to_owned(), caps)?;
    let space_id = repo
        .create_owned_space(&SpaceCreation::new(
            10_000,
            local_m.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members = vec![local_m, peer_m];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let advanced = repo.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        10_000,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    seed_address(&mut repo, (space_id, advanced.chain()), &peer_key)?;
    Ok(peer_id)
}

struct RoundHarness {
    round_task: tokio::task::JoinHandle<Result<Option<ControlRoundOutcome>, ControlFailure>>,
    owner_gate_tx: oneshot::Sender<()>,
    hanging_dropped_rx: oneshot::Receiver<()>,
    hanging_started: Arc<AtomicBool>,
}

fn spawn_admitted_queue_round(
    f: &Fixture,
    request: ControlRoundRequest,
    (hanging_peer, peer_b_seq2): (EndpointId, Vec<u8>),
) -> RoundHarness {
    let (owner_id, peer_b_id, gen2_response) = (f.owner_id, f.peer_b_id, f.gen2_response.clone());
    let (owner_gate_tx, owner_gate_rx) = oneshot::channel::<()>();
    let owner_gate_rx = Arc::new(tokio::sync::Mutex::new(Some(owner_gate_rx)));
    let (hanging_dropped_tx, hanging_dropped_rx) = oneshot::channel::<()>();
    let hanging_dropped_tx = Arc::new(std::sync::Mutex::new(Some(hanging_dropped_tx)));
    let hanging_started = Arc::new(AtomicBool::new(false));
    let runner = ControlRoundRunner::new(
        f.actor.store.clone(),
        f.actor.endpoint.control_client(),
        Arc::clone(&f.actor.clock),
    );
    let round_task = tokio::spawn(runner.run_with_dialer(request, {
        let hanging_started = Arc::clone(&hanging_started);
        move |peer, _| {
            let gen2 = gen2_response.clone();
            let peer_b_seq2 = peer_b_seq2.clone();
            let owner_gate_rx = Arc::clone(&owner_gate_rx);
            let hanging_dropped_tx = Arc::clone(&hanging_dropped_tx);
            let hanging_started = Arc::clone(&hanging_started);
            async move {
                if peer == peer_b_id {
                    Ok::<Vec<u8>, std::io::Error>(peer_b_seq2)
                } else if peer == owner_id {
                    let gate = owner_gate_rx.lock().await.take();
                    if let Some(rx) = gate {
                        let _ = rx.await;
                    }
                    Ok::<Vec<u8>, std::io::Error>(gen2)
                } else {
                    assert_eq!(peer, hanging_peer);
                    let tx = hanging_dropped_tx
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    let _guard = DialDropGuard(tx);
                    hanging_started.store(true, Ordering::SeqCst);
                    pending::<Result<Vec<u8>, std::io::Error>>().await
                }
            }
        }
    }));
    RoundHarness {
        round_task,
        owner_gate_tx,
        hanging_dropped_rx,
        hanging_started,
    }
}

async fn verify_post_release_projection(f: &mut Fixture) -> TestResult {
    let chain = f
        .actor
        .store
        .load_space_chain(f.space_id)
        .await?
        .ok_or("chain missing")?;
    assert_eq!(
        chain.latest_generation(),
        1,
        "queued response started applying after the round deadline expired"
    );

    f.actor.reconcile_control_completion().await?;
    let triggers = f
        .actor
        .control_schedule_events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        triggers.contains(&ControlRoundTrigger::AddressAdvanced),
        "earlier committed address change did not reach the actor"
    );
    assert!(
        !triggers.contains(&ControlRoundTrigger::ManifestAdvanced),
        "skipped post-deadline response must not trigger ManifestAdvanced"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn admitted_response_skips_apply_and_stops_dials_when_deadline_fires() -> TestResult {
    const SEED: u8 = 0x71;
    let mut f = setup_fixture("admitted-queue", SEED).await?;
    let hanging_peer = seed_hanging_peer(&f.local_dir.0, f.actor.state.endpoint_id, 0x91)?;
    let peer_b_seq2 = sign_address_seq2(f.space_id, SEED.wrapping_add(1))?;

    let (reply, mut waiter) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply))
        .ok_or("no global round")?;
    assert!(
        f.actor
            .control_queue
            .request(ControlRoundScope::peer(f.owner_id), None)
            .is_none(),
        "follow-up round must queue behind active round"
    );
    let request = ControlRoundRequest {
        local_endpoint_id: f.actor.state.endpoint_id,
        rotation: global.rotation(),
        now_ms: 10_000,
        scope: global.scope().clone(),
    };
    let mut harness = spawn_admitted_queue_round(&f, request, (hanging_peer, peer_b_seq2));

    wait_for_address_sequence(&f.actor.store, (f.space_id, f.peer_b_id), 2).await?;
    while !harness.hanging_started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    let committed_rev = f.actor.store.revision().await?;

    // Hold the Store thread while keeping its queue free, then let owner_id reply.
    let release_store = hold_store(&f.actor.store).await?;
    let baseline_capacity = f.actor.store.sender.capacity();
    let _ = harness.owner_gate_tx.send(());
    while f.actor.store.sender.capacity() == baseline_capacity {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        f.actor.store.sender.capacity(),
        baseline_capacity - 1,
        "response must enter the Store queue before the deadline test advances"
    );
    assert!(
        harness.hanging_dropped_rx.try_recv().is_err(),
        "hanging dial must still be active before deadline"
    );

    // Advance past the round deadline while the Store thread is still held.
    tokio::time::advance(CONTROL_ROUND_DEADLINE).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(
        harness.hanging_dropped_rx.try_recv().is_ok(),
        "outstanding dials must stop immediately when the round deadline fires"
    );

    assert!(
        harness.round_task.is_finished(),
        "expired round stayed blocked behind queued Store command"
    );
    let outcome = harness.round_task.await?;
    finish_round_without_blocking(&mut f.actor, global, outcome).await?;
    assert!(
        waiter.try_recv()?.is_err(),
        "explicit waiter must receive failure before Store blocker is released"
    );
    assert!(
        !f.actor.control_rounds.is_empty(),
        "follow-up round must launch before Store blocker is released"
    );
    assert!(
        f.actor.state.revision >= committed_rev,
        "earlier committed response revision did not reach the actor"
    );

    drop(release_store);
    verify_post_release_projection(&mut f).await?;

    f.actor.control_rounds.abort_all();
    while f.actor.control_rounds.join_next().await.is_some() {}
    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}
