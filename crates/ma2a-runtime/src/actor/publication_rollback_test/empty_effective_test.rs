use std::{
    sync::{Arc, atomic::AtomicI64},
    time::Duration,
};

use ma2a_core::{
    ControlArtifactKind, ControlCursorV1, ControlRequestV1, ControlResponseV1, EndpointId,
    InviteEntropy, RequestId, SpaceId,
};
use ma2a_store::{Repository, StoreConfig};

use super::{MutableClock, NOW_MS, TempState, TestResult, enabled_configuration};
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeHandle, control_sync};

fn respond_has_relay_advertisement(
    config: &StoreConfig,
    (local_id, remote_id, space_id): (EndpointId, EndpointId, SpaceId),
    now_ms: i64,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut repository = Repository::open(config)?;
    let spaces = repository.control_spaces_for(local_id)?;
    let space = spaces
        .into_iter()
        .find(|space| space.chain().space_id() == space_id)
        .ok_or("shared Space missing in Store")?;
    let cursor = ControlCursorV1::new(
        space_id,
        space.chain().latest_generation(),
        space.chain().latest_hash(),
    );
    let outcome = control_sync::respond(
        &mut repository,
        &control_sync::ControlExchangeInput {
            local_endpoint_id: local_id,
            remote_endpoint_id: remote_id,
            payload: ControlRequestV1::new(vec![cursor])?.encode()?,
            now_ms: u64::try_from(now_ms)?,
        },
    )
    .map_err(|failure| failure.to_string())?;
    let response_bytes = outcome.response.map_err(|failure| failure.to_string())?;
    let response = ControlResponseV1::decode(&response_bytes)?;
    Ok(response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .any(|artifact| artifact.kind() == ControlArtifactKind::RELAY_ADVERTISEMENT))
}

pub(super) async fn enroll_peer_in_space(
    (owner, peer): (&RuntimeHandle, &RuntimeHandle),
    space_id: SpaceId,
    request_byte: u8,
) -> TestResult {
    let ticket = owner
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    peer.clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([request_byte; 16].as_slice())?,
            "peer".to_owned(),
        ))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), owner.sync_control()).await??;
    Ok(())
}

async fn assert_advertisement_inactive(
    config: &StoreConfig,
    handle: &RuntimeHandle,
    (endpoint_id, peer_id, space_id): (EndpointId, EndpointId, SpaceId),
) -> TestResult {
    assert!(
        !Repository::open(config)?
            .relay_advertisement(space_id, endpoint_id)?
            .ok_or("removed advertisement missing")?
            .is_active()
    );
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/private_relay_candidates"),
        Some(&serde_json::json!([]))
    );
    assert!(!respond_has_relay_advertisement(
        config,
        (endpoint_id, peer_id, space_id),
        NOW_MS,
    )?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconfiguring_to_only_unjoined_space_deactivates_removed_advertisement_and_survives_restart()
-> TestResult {
    let state = TempState::new("empty-reconfig")?;
    let peer_state = TempState::new("empty-reconfig-peer")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let peer = Runtime::start_with_clock(
        StoreConfig::new(&peer_state.0),
        Arc::<MutableClock>::clone(&clock),
    )
    .await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    let peer_id = peer.handle().status().await?.endpoint_id();
    let owned_space = handle.create_owned_space("owned".to_owned()).await?;
    enroll_peer_in_space((&handle, &peer.handle()), owned_space, 0xa1).await?;

    handle
        .set_relay_configuration(enabled_configuration(vec![owned_space]))
        .await?;
    assert!(
        Repository::open(&config)?
            .relay_advertisement(owned_space, endpoint_id)?
            .is_some_and(|ad| ad.is_active())
    );
    assert!(respond_has_relay_advertisement(
        &config,
        (endpoint_id, peer_id, owned_space),
        NOW_MS,
    )?);

    let unjoined = SpaceId::try_from([0xbb; 32].as_slice())?;
    handle
        .set_relay_configuration(enabled_configuration(vec![unjoined]))
        .await?;
    assert!(!handle.relay_status().await?.convergence_pending);
    assert_advertisement_inactive(&config, &handle, (endpoint_id, peer_id, owned_space)).await?;

    tokio::time::timeout(Duration::from_secs(10), runtime.shutdown()).await??;
    let restarted =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let restarted_handle = restarted.handle();
    let _status = restarted_handle.status().await?;
    assert_advertisement_inactive(
        &config,
        &restarted_handle,
        (endpoint_id, peer_id, owned_space),
    )
    .await?;

    tokio::time::timeout(Duration::from_secs(10), peer.shutdown()).await??;
    tokio::time::timeout(Duration::from_secs(10), restarted.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_with_empty_effective_set_deactivates_stale_active_advertisement() -> TestResult {
    let state = TempState::new("empty-startup")?;
    let peer_state = TempState::new("empty-startup-peer")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let peer = Runtime::start_with_clock(
        StoreConfig::new(&peer_state.0),
        Arc::<MutableClock>::clone(&clock),
    )
    .await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    let peer_id = peer.handle().status().await?.endpoint_id();
    let owned_space = handle.create_owned_space("owned".to_owned()).await?;
    enroll_peer_in_space((&handle, &peer.handle()), owned_space, 0xa2).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![owned_space]))
        .await?;
    assert!(
        Repository::open(&config)?
            .relay_advertisement(owned_space, endpoint_id)?
            .is_some_and(|ad| ad.is_active())
    );
    tokio::time::timeout(Duration::from_secs(10), runtime.shutdown()).await??;

    // Replace the desired served set in SQLite with an unjoined Space while stopped so
    // startup observes an empty effective set alongside a persisted active advertisement.
    Repository::open(&config)?.set_relay_configuration(&enabled_configuration(vec![
        SpaceId::try_from([0xcc; 32].as_slice())?,
    ]))?;

    let restarted =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let restarted_handle = restarted.handle();
    let _status = restarted_handle.status().await?;
    assert_advertisement_inactive(
        &config,
        &restarted_handle,
        (endpoint_id, peer_id, owned_space),
    )
    .await?;

    tokio::time::timeout(Duration::from_secs(10), peer.shutdown()).await??;
    tokio::time::timeout(Duration::from_secs(10), restarted.shutdown()).await??;
    Ok(())
}
