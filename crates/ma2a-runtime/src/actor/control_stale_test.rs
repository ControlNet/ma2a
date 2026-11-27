use std::sync::Arc;

use ma2a_core::{
    EndpointId, MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig, StoreError};
use tokio::sync::mpsc;

use super::{Actor, maintenance::FaultPoint, relay_lifecycle_test::TempState};
use crate::{
    control_sync::{ControlChanges, ControlRoundOutcome, ControlRoundScope, ControlRoundTrigger},
    error::RuntimeError,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct StaleFixture {
    _directory: TempState,
    actor: Actor,
    store_task: tokio::task::JoinHandle<()>,
    space_id: SpaceId,
    local: SpaceMemberV1,
    member_b: SpaceMemberV1,
    member_c: SpaceMemberV1,
    peer_b: EndpointId,
    gen2_rev: u64,
}

async fn setup_fixture(label: &str, seed: u8) -> TestResult<StaleFixture> {
    let directory = TempState::new(label)?;
    let config = StoreConfig::new(&directory.0);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
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
        boot_id: [0x58; 16],
        revision: store.revision().await?,
        memberships: std::collections::BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let (mut actor, _, _) = Actor::new(
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
    let peer_b = EndpointSecret::parse(&[seed; 32])?.endpoint_id();
    let peer_c = EndpointSecret::parse(&[seed + 1; 32])?.endpoint_id();
    let caps = MemberCapabilities::new(true, true);
    let local = SpaceMemberV1::new(actor.state.endpoint_id, "owner".to_owned(), caps)?;
    let member_b = SpaceMemberV1::new(peer_b, "peer-b".to_owned(), caps)?;
    let member_c = SpaceMemberV1::new(peer_c, "peer-c".to_owned(), caps)?;
    let space_id = Repository::open(&config)?
        .create_owned_space(&SpaceCreation::new(
            10,
            local.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    actor.observe_memberships(vec![space_id]).await?;
    drain_rounds(&mut actor).await?;
    let gen2 = sorted_membership(vec![local.clone(), member_b.clone()]);
    let gen2_rev = actor
        .advance_owned_space(OwnedSpaceUpdate::new(space_id, 20, gen2))
        .await?;
    drain_rounds(&mut actor).await?;
    Ok(StaleFixture {
        _directory: directory,
        actor,
        store_task,
        space_id,
        local,
        member_b,
        member_c,
        peer_b,
        gen2_rev,
    })
}

async fn drain_rounds(actor: &mut Actor) -> TestResult {
    while let Some(joined) = actor.control_rounds.join_next().await {
        actor.finish_control_round(joined).await?;
    }
    Ok(())
}

fn sorted_membership(mut members: Vec<SpaceMemberV1>) -> SpaceManifestMembership {
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    SpaceManifestMembership::new(members, Vec::new())
}

fn outcome(revision: u64, peer: EndpointId, address: bool) -> ControlRoundOutcome {
    ControlRoundOutcome {
        revision,
        dialed_peers: [peer].into(),
        synchronized_peers: [peer].into(),
        changes: ControlChanges {
            manifest: false,
            address,
            relay: false,
        },
        incomplete: false,
    }
}

#[tokio::test]
async fn delayed_control_round_across_membership_advance_does_not_mark_peer_synchronized()
-> TestResult {
    let mut f = setup_fixture("outbound-stale", 0x21).await?;
    let stale_round = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no stale round")?;
    let gen3 = sorted_membership(vec![
        f.local.clone(),
        f.member_b.clone(),
        f.member_c.clone(),
    ]);
    let gen3_rev = f
        .actor
        .advance_owned_space(OwnedSpaceUpdate::new(f.space_id, 30, gen3))
        .await?;
    assert!(!f.actor.synchronized_control_peers.contains(&f.peer_b));
    let before_addr = f
        .actor
        .control_schedule_events
        .lock()
        .unwrap()
        .iter()
        .filter(|t| **t == ControlRoundTrigger::AddressAdvanced)
        .count();

    f.actor
        .finish_control_round(Ok((
            stale_round,
            Ok(Some(outcome(f.gen2_rev, f.peer_b, true))),
        )))
        .await?;

    assert!(f.actor.state.revision >= gen3_rev);
    let after_addr = f
        .actor
        .control_schedule_events
        .lock()
        .unwrap()
        .iter()
        .filter(|t| **t == ControlRoundTrigger::AddressAdvanced)
        .count();
    assert!(after_addr > before_addr);
    assert!(
        !f.actor.synchronized_control_peers.contains(&f.peer_b),
        "stale control round from before membership change re-marked peer B synchronized"
    );

    drain_rounds(&mut f.actor).await?;
    let current_round = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no current round")?;
    f.actor
        .finish_control_round(Ok((
            current_round,
            Ok(Some(outcome(gen3_rev, f.peer_b, false))),
        )))
        .await?;
    assert!(f.actor.synchronized_control_peers.contains(&f.peer_b));
    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}

#[tokio::test]
async fn failed_control_completion_retried_after_membership_advance_does_not_mark_peer_synchronized()
-> TestResult {
    let mut f = setup_fixture("pending-retry-stale", 0x23).await?;
    let round = f
        .actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no round")?;
    f.actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    f.actor
        .finish_control_round(Ok((round, Ok(Some(outcome(f.gen2_rev, f.peer_b, false))))))
        .await?;

    let gen3 = sorted_membership(vec![f.local, f.member_b, f.member_c]);
    f.actor
        .advance_owned_space(OwnedSpaceUpdate::new(f.space_id, 30, gen3))
        .await?;
    f.actor.reconcile_membership_completion().await?;

    assert!(!f.actor.synchronized_control_peers.contains(&f.peer_b));
    f.actor.finish(false).await?;
    f.store_task.await?;
    Ok(())
}
