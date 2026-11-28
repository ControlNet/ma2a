use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ma2a_net::CONTROL_ROUND_DEADLINE;
use tokio::sync::oneshot;

use super::fixture::{TestResult, setup_fixture};
use crate::{
    control_sync::{ControlRoundRequest, ControlRoundRunner, ControlRoundScope},
    store::{ServiceStoreCommand, StoreClient, StoreCommand},
};

async fn hold_store(store: &StoreClient) -> TestResult<std::sync::mpsc::SyncSender<()>> {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(0);
    store
        .sender
        .send(StoreCommand::Services(ServiceStoreCommand::Process(
            Box::new(move |_| {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
            }),
        )))
        .await
        .map_err(crate::store::channel_error)?;
    entered_rx.await?;
    Ok(release_tx)
}

#[tokio::test(start_paused = true)]
async fn no_effect_timeout_notifies_waiter_and_frees_slot_while_store_is_blocked() -> TestResult {
    let mut f = setup_fixture("no-effect-timeout", 0x77).await?;
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

    let (owner_gate_tx, owner_gate_rx) = oneshot::channel::<()>();
    let owner_gate_rx = Arc::new(tokio::sync::Mutex::new(Some(owner_gate_rx)));
    let started_dials = Arc::new(AtomicUsize::new(0));
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
    let owner_id = f.owner_id;
    let gen2_response = f.gen2_response.clone();
    let round_task = tokio::spawn(runner.run_with_dialer(request, {
        let started_dials = Arc::clone(&started_dials);
        move |peer, _| {
            let gen2 = gen2_response.clone();
            let owner_gate_rx = Arc::clone(&owner_gate_rx);
            let started_dials = Arc::clone(&started_dials);
            async move {
                started_dials.fetch_add(1, Ordering::SeqCst);
                if peer == owner_id {
                    let gate = owner_gate_rx.lock().await.take();
                    if let Some(rx) = gate {
                        let _ = rx.await;
                    }
                    Ok::<Vec<u8>, std::io::Error>(gen2)
                } else {
                    pending::<Result<Vec<u8>, std::io::Error>>().await
                }
            }
        }
    }));

    while started_dials.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    let release_store = hold_store(&f.actor.store).await?;
    let baseline_capacity = f.actor.store.sender.capacity();
    let _ = owner_gate_tx.send(());
    while f.actor.store.sender.capacity() == baseline_capacity {
        tokio::task::yield_now().await;
    }

    tokio::time::advance(CONTROL_ROUND_DEADLINE).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        round_task.is_finished(),
        "no-effect expired round stayed blocked behind queued Store command"
    );
    let outcome = round_task.await?;

    let finished = {
        let finish_fut = f.actor.finish_control_round(Ok((global, outcome)));
        tokio::pin!(finish_fut);
        let mut finished = None;
        for _ in 0..32 {
            tokio::select! {
                biased;
                res = &mut finish_fut => {
                    finished = Some(res);
                    break;
                }
                () = tokio::task::yield_now() => {}
            }
        }
        finished
    };
    finished.ok_or("finish_control_round stayed blocked in Store reconciliation")??;

    assert!(
        waiter.try_recv()?.is_err(),
        "explicit waiter must receive failure before Store blocker is released"
    );
    assert!(
        !f.actor.control_rounds.is_empty(),
        "follow-up round must launch immediately"
    );

    drop(release_store);
    f.actor.control_rounds.abort_all();
    while f.actor.control_rounds.join_next().await.is_some() {}
    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}
