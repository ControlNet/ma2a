use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::AtomicI64},
};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, MemberCapabilities, PrivateRelayAdvertisementScope,
    PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity, SpaceAuthorizationView,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    AdvertisementValidationContext, EndpointBindOptions, IrohHomeRelayObservation,
    IrohRelayObservation, PrivateRelayAdvertisementValidator, RelayUrl, RuntimeEndpoint,
    SpaceAddressLookup,
};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};
use tokio::sync::mpsc;

use super::{MutableClock, NOW_MS, TempState, TestResult};
use crate::{
    actor::Actor,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type ActorFixture = (
    TempState,
    StoreConfig,
    Arc<MutableClock>,
    Actor,
    tokio::task::JoinHandle<()>,
);

async fn build_actor(
    label: &str,
    initial_clock: i64,
) -> Result<ActorFixture, Box<dyn std::error::Error + Send + Sync>> {
    let state_dir = TempState::new(label)?;
    let config = StoreConfig::new(&state_dir.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(initial_clock)));
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let relay_map = store
        .load_relay_map(identity.endpoint_id, u64::try_from(initial_clock)?)
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
        boot_id: [0x61; 16],
        revision: store.revision().await?,
        memberships: BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    };
    let echo_metrics = endpoint.echo_metrics();
    let (actor, _handle, _cancellation) = Actor::new(
        status,
        endpoint,
        store,
        enrollment_calls,
        control_calls,
        echo_calls,
        echo_metrics,
        lookup,
        Arc::<MutableClock>::clone(&clock),
        None,
    );
    Ok((state_dir, config, clock, actor, store_task))
}

fn store_ad(
    repo: &mut Repository,
    auth: &SpaceAuthorizationView,
    (key, url): (&SecretKey, RelayUrl),
) -> TestResult {
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(auth.space_id(), key.public().into()),
        url,
        PrivateRelayAdvertisementValidity::new(
            1,
            u64::try_from(NOW_MS)?,
            u64::try_from(NOW_MS + 600_000)?,
        )?,
    )?
    .sign(key)?;
    PrivateRelayAdvertisementValidator::validate_and_store(
        repo,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(auth, u64::try_from(NOW_MS)?),
    )?;
    Ok(())
}

async fn stop_actor(actor: Actor, store_task: tokio::task::JoinHandle<()>) -> TestResult {
    let Actor {
        endpoint,
        store,
        relay_observer,
        ..
    } = actor;
    endpoint.shutdown().await?;
    relay_observer.await?;
    store.stop().await?;
    store_task.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_pending_iroh_observation_absorbs_clock_behind() -> TestResult {
    // Given: an Actor with a Space whose published address is ahead of its clock.
    let (_state_dir, _config, clock, mut actor, store_task) =
        build_actor("a06-obs-rollback", NOW_MS + 30_000).await?;
    let _space = actor.create_owned_space("ahead".to_owned()).await?;
    clock.0.store(NOW_MS, std::sync::atomic::Ordering::Relaxed);
    actor.state.direct_reachable = false;
    actor.state.endpoint_data = AddressEndpointDataV1::from_parts(Vec::new(), None)?;
    let observation = IrohRelayObservation::new(
        actor.state.endpoint_id,
        actor.endpoint.endpoint_data()?,
        Vec::new(),
    )?;
    let expected_data = observation.endpoint_data().clone();
    let expected_direct = observation.endpoint_addr().ip_addrs().next().is_some();
    assert!(
        expected_direct,
        "expected endpoint_addr to carry an IP address"
    );

    // When: an Iroh observation arrives with changed endpoint_data while the clock is behind.
    actor.observe_iroh_relay(observation).await?;

    // Then: deferred publication is absorbed, state updates, and pending_observation clears.
    assert!(actor.maintenance.pending_observation.is_none());
    assert_eq!(actor.state.direct_reachable, expected_direct);
    assert_eq!(actor.state.endpoint_data, expected_data);
    let snapshot = actor.snapshot().await?.to_value();
    assert_eq!(
        snapshot.pointer("/reachability/direct"),
        Some(&serde_json::json!(true))
    );

    stop_actor(actor, store_task).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_relay_candidates_keeps_partial_publish_revision_bump() -> TestResult {
    // Given: an Actor with two Spaces: `fresh` (due for renewal) and `blocked` (clock-behind).
    let (_state_dir, config, clock, mut actor, store_task) =
        build_actor("a06-partial-rev", NOW_MS - 400_000).await?;
    let relay = SecretKey::generate();
    let relay2 = SecretKey::generate();
    let primary_url: RelayUrl = "https://relay.example.invalid".parse()?;
    let backup_url: RelayUrl = "https://relay2.example.invalid".parse()?;
    let mut repository = Repository::open(&config)?;
    let local = SpaceMemberV1::new(
        actor.state.endpoint_id,
        "local".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let relay_cap = MemberCapabilities::new(true, true);
    let mut members = vec![
        local.clone(),
        SpaceMemberV1::new(relay.public().into(), "r1".to_owned(), relay_cap)?,
        SpaceMemberV1::new(relay2.public().into(), "r2".to_owned(), relay_cap)?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let membership = ma2a_core::SpaceManifestMembership::new(members, Vec::new());
    let fresh = repository
        .create_owned_space(&SpaceCreation::new(
            10,
            local.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let blocked = repository
        .create_owned_space(&SpaceCreation::new(
            10,
            local,
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    for space in [fresh, blocked] {
        repository.advance_owned_space(&ma2a_store::OwnedSpaceUpdate::new(
            space,
            11,
            membership.clone(),
        ))?;
    }
    drop(repository);

    actor.observe_memberships(vec![blocked, fresh]).await?;
    let publisher = actor.endpoint.address_publisher()?;
    let mut repository = Repository::open(&config)?;
    let auth = |space| -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
        let chain = repository.load_space_chain(space)?.ok_or("chain missing")?;
        Ok(SpaceAuthorizationView::from_chain(&chain))
    };
    let (fresh_auth, blocked_auth) = (auth(fresh)?, auth(blocked)?);
    clock.0.store(NOW_MS, std::sync::atomic::Ordering::Relaxed);
    store_ad(&mut repository, &fresh_auth, (&relay, primary_url.clone()))?;
    store_ad(
        &mut repository,
        &blocked_auth,
        (&relay, primary_url.clone()),
    )?;
    assert!(actor.refresh_relay_candidates().await?);
    let current_obs = IrohRelayObservation::new(
        actor.state.endpoint_id,
        actor.endpoint.endpoint_data()?,
        vec![IrohHomeRelayObservation::new(primary_url, true)],
    )?;
    actor.observe_iroh_relay(current_obs).await?;
    publisher
        .publish(
            &mut repository,
            ma2a_net::AddressPublishRequest::new(&blocked_auth, u64::try_from(NOW_MS + 30_000)?)
                .force_advance(),
        )?
        .ok_or("blocked did not advance")?;
    let db = rusqlite::Connection::open(config.database_path())?;
    db.execute(
        "DELETE FROM address_state WHERE space_id = ?1 AND endpoint_id = ?2",
        (
            fresh.as_bytes().as_slice(),
            actor.state.endpoint_id.as_bytes().as_slice(),
        ),
    )?;
    db.execute(
        "DELETE FROM relay_advertisement_state WHERE relay_endpoint_id = ?1",
        [relay.public().as_bytes().as_slice()],
    )?;
    store_ad(&mut repository, &fresh_auth, (&relay2, backup_url.clone()))?;
    store_ad(&mut repository, &blocked_auth, (&relay2, backup_url))?;
    actor.state.revision = repository.revision()?;
    let before = actor.state.revision;

    let changed = actor.refresh_relay_candidates().await?;
    let store_rev_after = repository.revision()?;
    assert!(changed);
    assert_eq!(actor.state.revision, store_rev_after);
    assert_eq!(store_rev_after, before + 2);

    stop_actor(actor, store_task).await
}
