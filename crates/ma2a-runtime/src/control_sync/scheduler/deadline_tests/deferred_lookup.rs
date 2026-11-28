use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlPageV1, ControlResponseV1, EndpointId, MemberCapabilities,
    SpaceAddressRecordV1, SpaceId, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
    default_member_label,
};
use ma2a_net::{
    AddressLookupClock, CONTROL_ROUND_DEADLINE, EndpointBindOptions, EndpointSecret,
    RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use tokio::sync::{mpsc, oneshot};

use super::fixture::{FixedClock, TempState, TestResult};
use crate::{
    actor::Actor,
    control_sync::{ControlChanges, ControlExchangeInput, ControlRoundOutcome, ControlRoundScope},
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

async fn commit_first_route(
    store: &StoreClient,
    (local_id, space_id): (EndpointId, SpaceId),
    (peer_key, data): (&SecretKey, AddressEndpointDataV1),
) -> TestResult<u64> {
    let peer_id = EndpointId::from(peer_key.public());
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(1, 10_000, 610_000)?,
        data,
    )
    .sign(peer_key)?;
    let artifact = ControlArtifactV1::new(
        ControlArtifactKind::ADDRESS_RECORD,
        signed.canonical_bytes().to_vec(),
    )?;
    let payload =
        ControlResponseV1::new(vec![ControlPageV1::new(space_id, false, vec![artifact])?])?
            .encode()?;
    let input = ControlExchangeInput {
        local_endpoint_id: local_id,
        remote_endpoint_id: peer_id,
        payload,
        now_ms: 10_000,
    };
    let _applied = store
        .admit_control_response(
            (input, vec![space_id]),
            tokio::time::Instant::now() + CONTROL_ROUND_DEADLINE,
        )
        .await?
        .response
        .await??;
    Ok(store.revision().await?)
}

#[derive(Debug)]
struct FixedLookupClock(u64);

impl AddressLookupClock for FixedLookupClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

async fn spawn_local_actor(
    config: &StoreConfig,
) -> TestResult<(Actor, tokio::task::JoinHandle<()>)> {
    let backend = StoreBackend::open(config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedLookupClock(10_000)));
    let map = store.load_relay_map(identity.endpoint_id, 10_000).await?;
    let (enroll_tx, enroll_rx) = mpsc::channel(1);
    let (ctrl_tx, ctrl_rx) = mpsc::channel(1);
    let (echo_tx, echo_rx) = mpsc::channel(1);
    let opts = EndpointBindOptions::new(enroll_tx, None)
        .with_control(ctrl_tx, false)
        .with_echo(echo_tx)
        .with_relay_map(map.clone());
    let endpoint = RuntimeEndpoint::bind_with_lookup(identity.secret, lookup.clone(), opts).await?;
    let state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x83; 16],
        revision: store.revision().await?,
        memberships: BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let metrics = endpoint.echo_metrics();
    let clock = Arc::new(FixedClock(10_000));
    let (actor, _, _) = Actor::new(
        state, endpoint, store, enroll_rx, ctrl_rx, echo_rx, metrics, lookup, clock, None,
    );
    Ok((actor, store_task))
}

fn seed_shared_space(
    config: &StoreConfig,
    local_id: EndpointId,
    peer_id: EndpointId,
) -> TestResult<(SpaceId, u64)> {
    let caps = MemberCapabilities::new(true, true);
    let local_m = SpaceMemberV1::new(local_id, default_member_label(local_id), caps)?;
    let peer_m = SpaceMemberV1::new(peer_id, "target".to_owned(), caps)?;
    let mut members = vec![local_m.clone(), peer_m];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut repo = Repository::open(config)?;
    let space_id = repo
        .create_owned_space(&SpaceCreation::new(
            10_000,
            local_m,
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    repo.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        10_000,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    Ok((space_id, repo.revision()?))
}

fn spawn_target_responder(
    space_id: SpaceId,
    mut peer_ctrl_rx: mpsc::Receiver<ma2a_net::ControlCall>,
    peer_calls: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<TestResult> {
    tokio::spawn(async move {
        while let Some(call) = peer_ctrl_rx.recv().await {
            let Some(call) = call.authorize() else {
                continue;
            };
            let Ok((_request, responder)) = call.request().await else {
                continue;
            };
            peer_calls.fetch_add(1, Ordering::SeqCst);
            let empty = ControlPageV1::new(space_id, false, Vec::new())?;
            responder.respond(Ok(ControlResponseV1::new(vec![empty])?.encode()?));
        }
        Ok(())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_targeted_round_uses_newly_committed_route_while_lookup_update_was_gated()
-> TestResult {
    let dir = TempState::new("deferred-lookup")?;
    let config = StoreConfig::new(&dir.0);
    let (mut actor, store_task) = spawn_local_actor(&config).await?;
    let local_id = actor.state.endpoint_id;

    let peer_seed = [0x84; 32];
    let peer_key = SecretKey::from_bytes(&peer_seed);
    let peer_secret = EndpointSecret::parse(&peer_seed)?;
    let peer_id = peer_secret.endpoint_id();
    let (peer_enroll_tx, _peer_enroll_rx) = mpsc::channel(1);
    let (peer_ctrl_tx, peer_ctrl_rx) = mpsc::channel(8);
    let peer_endpoint = RuntimeEndpoint::bind_with_lookup(
        peer_secret,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(peer_enroll_tx, None).with_control(peer_ctrl_tx, true),
    )
    .await?;
    let peer_data = peer_endpoint.endpoint_data()?;

    let (space_id, seeded_rev) = seed_shared_space(&config, local_id, peer_id)?;
    actor.state.revision = actor.state.revision.max(seeded_rev);
    actor.state.memberships.insert(space_id);
    actor.refresh_control_lookup().await?;
    assert!(
        actor
            .lookup
            .resolve_endpoint(peer_id.to_public_key()?)
            .is_none()
    );

    let peer_calls = Arc::new(AtomicUsize::new(0));
    let peer_task = spawn_target_responder(space_id, peer_ctrl_rx, Arc::clone(&peer_calls));

    let (reply1, mut waiter1) = oneshot::channel();
    let round1 = actor
        .control_queue
        .request(ControlRoundScope::all(), Some(reply1))
        .ok_or("missing round 1")?;
    let (reply2, mut waiter2) = oneshot::channel();
    assert!(
        actor
            .control_queue
            .request(ControlRoundScope::peer(peer_id), Some(reply2))
            .is_none()
    );

    // Commit peer_id's first route into Store as round 1's partial commit while lookup stays stale.
    let committed_rev =
        commit_first_route(&actor.store, (local_id, space_id), (&peer_key, peer_data)).await?;

    let partial_outcome = ControlRoundOutcome {
        revision: Some(committed_rev),
        dialed_peers: BTreeSet::new(),
        synchronized_peers: BTreeSet::new(),
        changes: ControlChanges {
            manifest: false,
            address: true,
            relay: false,
        },
        incomplete: true,
    };
    actor
        .finish_control_round(Ok((round1, Ok(Some(partial_outcome)))))
        .await?;

    assert!(waiter1.try_recv()?.is_err());
    assert!(matches!(
        waiter2.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(!actor.control_rounds.is_empty());

    // Keep the deferred lookup update gated across the window in which an unheld dial would
    // already fail with NoAddress on all retries.
    let premature =
        tokio::time::timeout(Duration::from_millis(500), actor.control_rounds.join_next()).await;
    assert!(
        premature.is_err(),
        "follow-up round dialed before lookup update: {premature:?}"
    );
    assert_eq!(peer_calls.load(Ordering::SeqCst), 0);

    // Release the lookup gate; the queued targeted round must now dial peer_id over the new route.
    actor.reconcile_control_completion().await?;
    let round2_result =
        tokio::time::timeout(CONTROL_ROUND_DEADLINE, actor.control_rounds.join_next())
            .await?
            .ok_or("missing follow-up round result")?;
    actor.finish_control_round(round2_result).await?;

    let reply2 = tokio::time::timeout(Duration::from_secs(2), &mut waiter2).await??;
    assert!(
        reply2.is_ok(),
        "follow-up round failed despite newly committed route: {reply2:?}"
    );
    assert!(peer_calls.load(Ordering::SeqCst) >= 1);

    drop(peer_endpoint);
    peer_task.abort();
    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}
