use std::sync::Arc;

use iroh::SecretKey;
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceAuthorizationView, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    AdvertisementValidationContext, EndpointBindOptions, PrivateRelayAdvertisementValidator,
    RelayUrl, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use tokio::sync::mpsc;

use super::{FixedClock, NOW_MS, TempState, TestResult};
use crate::{
    actor::Actor,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_relay_refresh_automatically_publishes_next_address_record() -> TestResult {
    let state_dir = TempState::new()?;
    let config = StoreConfig::new(state_dir.path());
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let relay = SecretKey::generate();
    let relay_url: RelayUrl = "https://relay.example.invalid".parse()?;
    let mut repository = Repository::open(&config)?;
    let authorization =
        create_space_with_relay(&mut repository, identity.endpoint_id, relay.public().into())?;
    let initial_map = store
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
            .with_relay_map(initial_map.clone()),
    )
    .await?;
    let status = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x52; 16],
        revision: repository.revision()?,
        memberships: repository.memberships_for(identity.endpoint_id)?,
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        relay: crate::reachability::RelayReachabilityState::new(initial_map),
    };
    let echo_metrics = endpoint.echo_metrics();
    let (mut actor, _handle, _cancellation) = Actor::new(
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
    let _initial_observation = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        actor.relay_observations.recv(),
    )
    .await?
    .ok_or("initial relay observation channel closed")?;
    let publisher = actor.endpoint.address_publisher()?;
    let (_revision, advanced) = actor
        .store
        .publish_address(
            publisher,
            identity.endpoint_id,
            u64::try_from(NOW_MS)?,
            false,
        )
        .await?;
    assert!(advanced);
    let initial = repository
        .address_record(authorization.space_id(), identity.endpoint_id)?
        .ok_or("initial address record missing")?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &authorization,
            relay: &relay,
            relay_url,
        },
    )?;

    let changed = actor.refresh_relay_candidates().await?;
    let updated = repository
        .address_record(authorization.space_id(), identity.endpoint_id)?
        .ok_or("updated address record missing")?;

    assert!(changed);
    assert_eq!(updated.sequence(), initial.sequence() + 1);
    actor.control_rounds.shutdown().await;
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

fn create_space_with_relay(
    repository: &mut Repository,
    local_endpoint_id: ma2a_core::EndpointId,
    relay_endpoint_id: ma2a_core::EndpointId,
) -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let local = SpaceMemberV1::new(
        local_endpoint_id,
        "local".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let relay = SpaceMemberV1::new(
        relay_endpoint_id,
        "relay".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        local.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local, relay];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        11,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("created Space chain is missing")?;
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

struct AdvertisementFixture<'a> {
    authorization: &'a SpaceAuthorizationView,
    relay: &'a SecretKey,
    relay_url: RelayUrl,
}

fn store_advertisement(
    repository: &mut Repository,
    fixture: AdvertisementFixture<'_>,
) -> TestResult {
    let now_ms = u64::try_from(NOW_MS)?;
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(
            fixture.authorization.space_id(),
            fixture.relay.public().into(),
        ),
        fixture.relay_url,
        PrivateRelayAdvertisementValidity::new(1, now_ms, now_ms + 600_000)?,
    )?
    .sign(fixture.relay)?;
    PrivateRelayAdvertisementValidator::validate_and_store(
        repository,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(fixture.authorization, now_ms),
    )?;
    Ok(())
}
