mod admitted_projection;
mod admitted_queue;
mod dialed_peers;
mod fixture;
mod no_effect_timeout;
mod store_queue;

use std::{future::pending, sync::Arc};

use tokio::sync::oneshot;

use fixture::{FailSecondClock, Fixture, TestResult, setup_fixture};

use crate::{
    RuntimeError,
    control_sync::{
        ControlFailure, ControlRoundOutcome, ControlRoundRequest, ControlRoundRunner,
        ControlRoundScope, ControlRoundTrigger,
    },
    error::RuntimeErrorKind,
};

struct DialDropGuard(Option<oneshot::Sender<()>>);

impl Drop for DialDropGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

async fn wait_for_generation(
    store: &crate::store::StoreClient,
    space_id: ma2a_core::SpaceId,
    expected: u64,
) -> TestResult {
    while store
        .load_space_chain(space_id)
        .await?
        .is_none_or(|chain| chain.latest_generation() < expected)
    {
        tokio::task::yield_now().await;
    }
    Ok(())
}

async fn run_failing_second_peer(
    f: &Fixture,
    (rotation, scope): (usize, ControlRoundScope),
    error: fn() -> RuntimeError,
) -> TestResult<Result<Option<ControlRoundOutcome>, ControlFailure>> {
    let (owner_id, gen2_response, empty_response) = (
        f.owner_id,
        f.gen2_response.clone(),
        f.empty_response.clone(),
    );
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let runner = ControlRoundRunner::new(
        f.actor.store.clone(),
        f.actor.endpoint.control_client(),
        Arc::new(FailSecondClock::new(error)),
    );
    let request = ControlRoundRequest {
        local_endpoint_id: f.actor.state.endpoint_id,
        rotation,
        now_ms: 10_000,
        scope,
    };
    let round_task = tokio::spawn(runner.run_with_dialer(request, move |peer, _| {
        let (gen2, empty) = (gen2_response.clone(), empty_response.clone());
        let rx = Arc::clone(&release_rx);
        async move {
            if peer == owner_id {
                Ok::<Vec<u8>, std::io::Error>(gen2)
            } else {
                let wait = rx.lock().await.take();
                if let Some(rx) = wait {
                    let _ = rx.await;
                }
                Ok::<Vec<u8>, std::io::Error>(empty)
            }
        }
    }));
    wait_for_generation(&f.actor.store, f.space_id, 2).await?;
    let _ = release_tx.send(());
    Ok(round_task.await?)
}

#[tokio::test(start_paused = true)]
async fn deadline_after_manifest_commit_drops_removed_peer_from_synchronized_view() -> TestResult {
    let mut f = setup_fixture("deadline-manifest", 0x51).await?;
    let (reply, waiter) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply))
        .ok_or("no global round")?;
    let owner_id = f.owner_id;
    let gen2_response = f.gen2_response.clone();

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
        let response = gen2_response.clone();
        async move {
            if peer == owner_id {
                Ok::<Vec<u8>, std::io::Error>(response)
            } else {
                pending::<Result<Vec<u8>, std::io::Error>>().await
            }
        }
    }));

    wait_for_generation(&f.actor.store, f.space_id, 2).await?;
    tokio::time::advance(ma2a_net::CONTROL_ROUND_DEADLINE).await;
    let outcome = round_task.await?;

    let committed_rev = f.actor.store.revision().await?;
    let chain = f
        .actor
        .store
        .load_space_chain(f.space_id)
        .await?
        .ok_or("chain missing")?;
    assert_eq!(chain.latest_generation(), 2);

    f.actor.finish_control_round(Ok((global, outcome))).await?;

    assert!(waiter.await?.is_err(), "aborted round must fail its waiter");
    let last_round = f.actor.control_round_history.back().ok_or("no history")?;
    assert_eq!(last_round.outcome, "failed");
    assert!(f.actor.state.revision >= committed_rev);
    assert!(
        !f.actor.synchronized_control_peers.contains(&f.peer_b_id),
        "removed peer stayed in synchronized_control_peers after deadline-aborted round"
    );
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

#[tokio::test]
async fn local_error_after_manifest_commit_keeps_changes_or_propagates_fatal() -> TestResult {
    // Retryable local error on the second peer still carries committed manifest changes.
    let mut f = setup_fixture("local-retryable", 0x55).await?;
    let (reply, waiter) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply))
        .ok_or("no global round")?;
    let outcome = run_failing_second_peer(&f, (global.rotation(), global.scope().clone()), || {
        RuntimeError::new(RuntimeErrorKind::ClockBehind)
    })
    .await?;

    let committed_rev = f.actor.store.revision().await?;
    f.actor.finish_control_round(Ok((global, outcome))).await?;
    assert!(waiter.await?.is_err());
    assert!(f.actor.state.revision >= committed_rev);
    assert!(
        !f.actor.synchronized_control_peers.contains(&f.peer_b_id),
        "removed peer stayed in synchronized_control_peers after retryable local error"
    );
    f.actor.finish(false).await?;
    f.store_task.await?;

    // Fatal local error on the second peer still surfaces as ControlFailure::Runtime.
    let mut fatal = setup_fixture("local-fatal", 0x59).await?;
    let global = fatal
        .actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no global round")?;
    let outcome =
        run_failing_second_peer(&fatal, (global.rotation(), global.scope().clone()), || {
            RuntimeError::new(RuntimeErrorKind::Clock)
        })
        .await?;
    assert!(matches!(outcome, Err(ControlFailure::Runtime(_))));
    fatal.actor.finish(false).await?;
    fatal.store_task.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn running_store_apply_keeps_receipt_while_stopping_dials_at_deadline() -> TestResult {
    let mut dials = tokio::task::JoinSet::<super::DialOutcome<std::io::Error>>::new();
    let (dial_dropped_tx, mut dial_dropped_rx) = oneshot::channel::<()>();
    let (dial_started_tx, dial_started_rx) = oneshot::channel::<()>();
    let peer = ma2a_net::EndpointSecret::parse(&[0x44; 32])?.endpoint_id();
    dials.spawn(async move {
        let _guard = DialDropGuard(Some(dial_dropped_tx));
        let _ = dial_started_tx.send(());
        pending::<()>().await;
        (peer, Vec::new(), Ok(Vec::new()))
    });
    dial_started_rx.await?;

    let (reply_tx, reply_rx) = oneshot::channel();
    let lease = crate::store_client::control::ControlApplyLease::new();
    assert!(lease.begin());
    let wait_task = tokio::spawn(async move {
        let deadline = tokio::time::sleep(ma2a_net::CONTROL_ROUND_DEADLINE);
        tokio::pin!(deadline);
        super::await_store_apply_receipt(
            &mut deadline,
            &mut dials,
            crate::store_client::control::AdmittedControlApply {
                response: reply_rx,
                lease,
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(dial_dropped_rx.try_recv().is_err());

    tokio::time::advance(ma2a_net::CONTROL_ROUND_DEADLINE).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(
        dial_dropped_rx.try_recv().is_ok(),
        "outstanding dials must stop when deadline fires during a running Store apply"
    );

    let outcome = crate::control_sync::ControlApplyOutcome {
        revision: 42,
        changes: crate::control_sync::ControlChanges {
            manifest: true,
            ..crate::control_sync::ControlChanges::default()
        },
    };
    let _ = reply_tx.send(Ok(outcome));
    let received = wait_task.await??;
    let applied = received?;
    assert_eq!(applied.revision, 42);
    assert!(applied.changes.manifest);
    Ok(())
}
