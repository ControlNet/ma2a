use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlPageV1, ControlResponseV1, EndpointId, SpaceAddressRecordV1, SpaceId,
};
use ma2a_net::CONTROL_ROUND_DEADLINE;
use tokio::sync::oneshot;

use super::fixture::{Fixture, TestResult, setup_fixture};
use crate::{
    control_sync::{ControlFailure, ControlRoundOutcome, ControlRoundScope, ControlRoundTrigger},
    store::{ServiceStoreCommand, StoreClient, StoreCommand},
};

pub(super) fn sign_address_seq2(space_id: SpaceId, peer_seed: u8) -> TestResult<Vec<u8>> {
    let peer_key = iroh::SecretKey::from_bytes(&[peer_seed; 32]);
    let peer_id = EndpointId::from(peer_key.public());
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(2, 10_000, 610_000)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(&peer_key)?;
    let artifact = ControlArtifactV1::new(
        ControlArtifactKind::ADDRESS_RECORD,
        signed.canonical_bytes().to_vec(),
    )?;
    let page = ControlPageV1::new(space_id, false, vec![artifact])?;
    Ok(ControlResponseV1::new(vec![page])?.encode()?)
}

pub(super) async fn wait_for_address_sequence(
    store: &StoreClient,
    (space_id, peer_id): (SpaceId, EndpointId),
    expected: u64,
) -> TestResult {
    while store
        .address_record(space_id, peer_id)
        .await?
        .is_none_or(|record| record.sequence() < expected)
    {
        tokio::task::yield_now().await;
    }
    Ok(())
}

pub(super) async fn hold_store(store: &StoreClient) -> TestResult<std::sync::mpsc::SyncSender<()>> {
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

pub(super) async fn finish_round_without_blocking(
    actor: &mut crate::actor::Actor,
    global: crate::control_actor::ScheduledControlRound,
    outcome: Result<Option<ControlRoundOutcome>, ControlFailure>,
) -> TestResult {
    let finish_fut = actor.finish_control_round(Ok((global, outcome)));
    tokio::pin!(finish_fut);
    for _ in 0..32 {
        tokio::select! {
            biased;
            res = &mut finish_fut => return Ok(res?),
            () = tokio::task::yield_now() => {}
        }
    }
    Err("finish_control_round stayed blocked in Store reconciliation".into())
}

async fn commit_partial_response(
    f: &Fixture,
    remote_endpoint_id: EndpointId,
    payload: Vec<u8>,
) -> TestResult<u64> {
    let _applied = f
        .actor
        .store
        .admit_control_response(
            (
                crate::control_sync::ControlExchangeInput {
                    local_endpoint_id: f.actor.state.endpoint_id,
                    remote_endpoint_id,
                    payload,
                    now_ms: 10_000,
                },
                vec![f.space_id],
            ),
            tokio::time::Instant::now() + CONTROL_ROUND_DEADLINE,
        )
        .await?
        .response
        .await??;
    Ok(f.actor.store.revision().await?)
}

#[tokio::test(start_paused = true)]
async fn partial_manifest_commit_keeps_followup_generation_and_projects_after_unblock() -> TestResult
{
    let mut f = setup_fixture("partial-manifest-proj", 0x73).await?;
    // Commit gen2 (revoking peer_b_id) in Store as the first peer's partial commit.
    let committed_rev = commit_partial_response(&f, f.owner_id, f.gen2_response.clone()).await?;

    let (reply1, mut waiter1) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply1))
        .ok_or("no global round")?;
    let (reply2, mut waiter2) = oneshot::channel();
    assert!(
        f.actor
            .control_queue
            .request(ControlRoundScope::peer(f.owner_id), Some(reply2))
            .is_none()
    );
    let before_view = f.actor.view_sequence;

    let release_store = hold_store(&f.actor.store).await?;
    let partial_outcome = ControlRoundOutcome {
        revision: Some(committed_rev),
        dialed_peers: [f.owner_id].into(),
        synchronized_peers: std::collections::BTreeSet::new(),
        changes: crate::control_sync::ControlChanges {
            manifest: true,
            address: false,
            relay: false,
        },
        incomplete: true,
    };
    finish_round_without_blocking(&mut f.actor, global, Ok(Some(partial_outcome))).await?;

    // Before Store unblocks: waiter1 failed, view updated, peer_b cleared, follow-up launched.
    assert!(waiter1.try_recv()?.is_err());
    assert!(matches!(
        waiter2.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(f.actor.view_sequence > before_view);
    assert!(!f.actor.synchronized_control_peers.contains(&f.peer_b_id));
    assert!(!f.actor.control_rounds.is_empty());
    assert!(!f.actor.maintenance.control_projections.is_empty());
    let post_commit_generation = f.actor.synchronized_control_peers.generation();

    // After Store unblocks, the separate projection runs without invalidating the follow-up's
    // post-commit membership generation.
    drop(release_store);
    let projection_wake = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.actor.maintenance.control_projections.join_next(),
    )
    .await?
    .ok_or("missing projection wakeup")?;
    projection_wake?;
    f.actor.reconcile_control_completion().await?;

    assert_eq!(
        f.actor.synchronized_control_peers.generation(),
        post_commit_generation,
        "separate projection bumped generation again and invalidated follow-up round"
    );
    let triggers = f
        .actor
        .control_schedule_events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(triggers.contains(&ControlRoundTrigger::ManifestAdvanced));

    f.actor.control_rounds.abort_all();
    while f.actor.control_rounds.join_next().await.is_some() {}
    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn actor_run_loop_projects_partial_commit_promptly_after_store_unblocks() -> TestResult {
    const SEED: u8 = 0x75;
    let mut f = setup_fixture("actor-proj-loop", SEED).await?;
    let peer_b_seq2 = sign_address_seq2(f.space_id, SEED.wrapping_add(1))?;
    let committed_rev = commit_partial_response(&f, f.peer_b_id, peer_b_seq2).await?;

    let (reply, waiter) = oneshot::channel();
    let global = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply))
        .ok_or("no global round")?;
    let release_store = hold_store(&f.actor.store).await?;
    let partial_outcome = ControlRoundOutcome {
        revision: Some(committed_rev),
        dialed_peers: [f.peer_b_id].into(),
        synchronized_peers: std::collections::BTreeSet::new(),
        changes: crate::control_sync::ControlChanges {
            manifest: false,
            address: true,
            relay: false,
        },
        incomplete: true,
    };
    f.actor
        .control_rounds
        .spawn(async move { (global, Ok(Some(partial_outcome))) });

    let handle = f.handle;
    let actor_task = tokio::spawn(f.actor.run());

    // Explicit waiter must fail before the Store blocker is released.
    let waiter_result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter).await??;
    assert!(waiter_result.is_err());

    // Release the Store blocker; the Actor loop must run the deferred projection promptly
    // without advancing the paused clock to the 60s periodic tick.
    drop(release_store);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !handle
        .control_schedules()
        .contains(&ControlRoundTrigger::AddressAdvanced)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Actor::run_loop did not project partial commit after Store unblocked"
        );
        tokio::task::yield_now().await;
    }

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    handle
        .commands
        .send(crate::actor::Command::Shutdown(shutdown_tx))
        .await?;
    let _ack = tokio::time::timeout(std::time::Duration::from_secs(5), shutdown_rx).await??;
    actor_task.await??;
    f.store_task.await?;
    Ok(())
}
