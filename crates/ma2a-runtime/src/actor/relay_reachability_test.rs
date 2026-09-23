use std::{collections::BTreeSet, sync::Arc};

use iroh::SecretKey;
use ma2a_core::{AddressEndpointDataV1, RelayReachability};
use ma2a_net::{
    EndpointBindOptions, IrohHomeRelayObservation, IrohRelayObservation, RelayUrl, RuntimeEndpoint,
    SpaceAddressLookup,
};
use ma2a_store::{Repository, StoreConfig};
use tokio::sync::mpsc;

use super::{FixedClock, NOW_MS, TempState, TestResult};
#[path = "relay_reachability_fixture.rs"]
mod fixture;
use crate::{
    actor::Actor,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};
use fixture::{AdvertisementFixture, create_space_with_relay, store_advertisement};

#[tokio::test(start_paused = true)]
#[expect(
    clippy::too_many_lines,
    reason = "the integration scenario keeps setup, observation, persistence, and cleanup visible"
)]
async fn active_space_filters_connected_rogue_home_from_status_and_repository() -> TestResult {
    // Given
    let state_dir = TempState::new()?;
    let config = StoreConfig::new(state_dir.path());
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let relay = SecretKey::generate();
    let allowed_url: RelayUrl = "https://allowed.example.invalid".parse()?;
    let rogue_url: RelayUrl = "https://rogue.example.invalid".parse()?;
    let mut repository = Repository::open(&config)?;
    let authorization =
        create_space_with_relay(&mut repository, identity.endpoint_id, relay.public().into())?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &authorization,
            relay: &relay,
            relay_url: allowed_url.clone(),
        },
    )?;
    let relay_map = store
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
            .with_relay_map(relay_map.clone()),
    )
    .await?;
    let endpoint_data = endpoint.endpoint_data()?;
    let endpoint_addr = endpoint.endpoint_addr();
    let status = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr,
        endpoint_data: endpoint_data.clone(),
        boot_id: [0x51; 16],
        revision: repository.revision()?,
        memberships: repository.memberships_for(identity.endpoint_id)?,
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    };
    let awaiting = RelayReachability::AwaitingIrohHome;
    assert_eq!(status.relay_reachability(), awaiting);
    let echo_metrics = endpoint.echo_metrics();
    let (mut actor, handle, _cancellation) = Actor::new(
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
    let initial_snapshot = actor.snapshot().await?.to_value();
    assert_eq!(
        initial_snapshot.pointer("/reachability/direct"),
        Some(&serde_json::json!(false))
    );
    let observation = IrohRelayObservation::new(
        identity.endpoint_id,
        endpoint_data,
        vec![
            IrohHomeRelayObservation::new(allowed_url.clone(), true),
            IrohHomeRelayObservation::new(rogue_url.clone(), true),
        ],
    )?;

    // When
    // Deliberately stale Actor projection: the live Iroh snapshot must replace it.
    actor.state.endpoint_data = AddressEndpointDataV1::from_parts(Vec::new(), None)?;
    let before_revision = actor.state.revision;
    actor.maintenance.fail_once(
        crate::actor::maintenance::FaultPoint::EndpointObservationPersist,
        crate::error::RuntimeError::from(ma2a_store::StoreError::Io(
            std::io::ErrorKind::Interrupted.into(),
        )),
    );
    actor.maintenance.fail_once(
        crate::actor::maintenance::FaultPoint::AddressPublication,
        crate::error::RuntimeError::from(ma2a_store::StoreError::Io(
            std::io::ErrorKind::Interrupted.into(),
        )),
    );
    assert!(actor.observe_iroh_relay(observation.clone()).await.is_err());
    assert_eq!(
        actor.maintenance.pending_observation,
        Some(observation.clone())
    );
    assert_ne!(actor.state.endpoint_data, *observation.endpoint_data());
    assert_eq!(actor.state.revision, before_revision);
    assert!(actor.reconcile_pending_iroh_observation().await.is_err());
    assert_eq!(actor.state.revision, before_revision);
    let published_sequence = repository
        .address_record(authorization.space_id(), identity.endpoint_id)?
        .ok_or("address publication did not commit before lookup failure")?
        .sequence();
    actor.reconcile_pending_iroh_observation().await?;
    assert!(actor.maintenance.pending_observation.is_none());
    assert!(actor.state.revision > before_revision);
    assert_eq!(
        repository
            .address_record(authorization.space_id(), identity.endpoint_id)?
            .ok_or("address publication missing after retry")?
            .sequence(),
        published_sequence
    );
    let observed_status = actor.state.clone();
    let persisted = repository.relay_observations()?;
    let snapshot = actor.snapshot().await?.to_value();

    // Then
    assert_eq!(
        observed_status.relay_reachability(),
        RelayReachability::IrohHomeConnected
    );
    let observed_homes = observed_status
        .observed_home_relays()
        .collect::<BTreeSet<_>>();
    assert_eq!(observed_homes, BTreeSet::from([allowed_url.as_str()]));
    assert!(
        persisted
            .iter()
            .all(|item| item.relay_url != rogue_url.as_str())
    );
    let allowed = persisted
        .iter()
        .find(|item| item.relay_url == allowed_url.as_str())
        .ok_or("allowed relay observation was not persisted")?;
    assert!(allowed.reachable);
    let active_space_ids = snapshot
        .pointer("/spaces")
        .and_then(serde_json::Value::as_array)
        .map(|spaces| {
            spaces
                .iter()
                .filter_map(|space| space.pointer("/space_id").cloned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!active_space_ids.is_empty());
    assert_eq!(
        snapshot.pointer("/private_relay_candidates"),
        Some(&serde_json::json!([{
            "provider_endpoint_id": crate::api::encode_hex(relay.public().as_bytes()),
            "relay_url": allowed_url.as_str(),
            "covered_space_ids": active_space_ids,
            "home_compatible": true,
        }]))
    );
    assert_eq!(
        snapshot.pointer("/observed_relay_state"),
        Some(&serde_json::json!({
            "private_relay_provider_running": false,
            "public_relay_connected": false,
        }))
    );
    // No local Private Relay Provider runs here, yet Iroh reports a connected
    // home. The two facts are independent and a client must never derive one
    // from the other.
    assert_eq!(
        snapshot.pointer("/reachability/state"),
        Some(&serde_json::json!("IrohHomeConnected"))
    );
    assert!(
        snapshot
            .pointer("/public_relay_fallbacks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert!(snapshot.pointer("/control_sync/synchronized").is_none());
    assert_eq!(
        snapshot.pointer("/reachability/direct"),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        snapshot.pointer("/reachability/relayed"),
        Some(&serde_json::json!(true))
    );

    // Both stream items are complete snapshots. A newer one replaces an older
    // failed desire and must not later be rolled back by the periodic retry.
    let old = IrohRelayObservation::new(
        identity.endpoint_id,
        observation.endpoint_data().clone(),
        vec![IrohHomeRelayObservation::new(allowed_url.clone(), false)],
    )?;
    let newer = IrohRelayObservation::new(
        identity.endpoint_id,
        observation.endpoint_data().clone(),
        Vec::new(),
    )?;
    for _ in 0..2 {
        actor.maintenance.fail_once(
            crate::actor::maintenance::FaultPoint::EndpointObservationPersist,
            crate::error::RuntimeError::from(ma2a_store::StoreError::Io(
                std::io::ErrorKind::Interrupted.into(),
            )),
        );
    }
    assert!(actor.observe_iroh_relay(old).await.is_err());
    assert!(actor.observe_iroh_relay(newer.clone()).await.is_err());
    assert_eq!(actor.maintenance.pending_observation, Some(newer));
    // Close the independent Iroh watcher channel so the test controls the
    // one-shot pending snapshot without another transport event superseding it.
    let (_sender, observations) = mpsc::channel(1);
    actor.relay_observations = observations;
    let mut events = handle.subscribe();
    let actor_task = tokio::spawn(actor.run());
    assert!(events.recv().await?.is_ready());
    tokio::task::yield_now().await;
    tokio::time::advance(crate::control_actor::control_period(identity.endpoint_id)).await;
    let mut status = handle.status().await?;
    for _ in 0..10_000 {
        if status.observed_home_relays().next().is_none() {
            break;
        }
        tokio::task::yield_now().await;
        status = handle.status().await?;
    }
    assert!(status.observed_home_relays().next().is_none());
    assert!(repository.relay_observations()?.is_empty());
    handle.shutdown().await?;
    actor_task.await??;
    store_task.await?;
    Ok(())
}
