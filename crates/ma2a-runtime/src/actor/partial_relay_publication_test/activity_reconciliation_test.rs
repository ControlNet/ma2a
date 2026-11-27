use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::{
    ControlArtifactKind, ControlCursorV1, ControlRequestV1, ControlResponseV1, EndpointId,
    MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreConfig};

use super::{
    NOW_MS, PublicationClock, TempState, TestResult, enabled_configuration, maintenance_tick,
    partial_relay_fixture,
};
use crate::{Runtime, RuntimeClock, control_sync};

pub(in crate::actor) fn add_peer(
    config: &StoreConfig,
    space_id: SpaceId,
    peer_id: EndpointId,
) -> TestResult {
    let mut repository = Repository::open(config)?;
    let chain = repository
        .load_space_chain(space_id)?
        .ok_or("Space chain missing")?;
    let mut members = chain.members().to_vec();
    members.push(SpaceMemberV1::new(
        peer_id,
        "peer".to_owned(),
        MemberCapabilities::new(true, false),
    )?);
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        u64::try_from(NOW_MS + 1)?,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    Ok(())
}

pub(in crate::actor) fn respond_has_relay_advertisement(
    config: &StoreConfig,
    (local_id, remote_id, space_id): (EndpointId, EndpointId, SpaceId),
    now_ms: u64,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut repository = Repository::open(config)?;
    let space = repository
        .control_spaces_for(local_id)?
        .into_iter()
        .find(|state| state.chain().space_id() == space_id)
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
            now_ms,
        },
    )
    .map_err(|failure| failure.to_string())?;
    let response = ControlResponseV1::decode(&outcome.response)?;
    Ok(response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .any(|artifact| artifact.kind() == ControlArtifactKind::RELAY_ADVERTISEMENT))
}

#[tokio::test(start_paused = true)]
async fn retained_partial_batch_clock_step_and_space_removal_deactivates_removed_spaces()
-> TestResult {
    let (_state, fixture) = partial_relay_fixture("clock-step-remove").await?;
    let peer_id = EndpointId::from(iroh::SecretKey::from_bytes(&[0x55; 32]).public());
    add_peer(&fixture.config, fixture.committed_space, peer_id)?;
    assert!(
        Repository::open(&fixture.config)?
            .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
            .is_some_and(|ad| ad.is_active())
    );
    assert!(respond_has_relay_advertisement(
        &fixture.config,
        (fixture.endpoint_id, peer_id, fixture.committed_space),
        u64::try_from(NOW_MS)?,
    )?);

    // Step the clock behind the retained batch, then remove its effective Spaces.
    let stepped_back_ms = NOW_MS - 1_000;
    fixture.clock.0.store(stepped_back_ms, Ordering::Relaxed);
    let unjoined = SpaceId::try_from([0xee; 32].as_slice())?;
    fixture
        .handle
        .set_relay_configuration(enabled_configuration(vec![unjoined]))
        .await?;
    assert!(!fixture.handle.store.relay_publication_pending().await?);
    assert!(
        !Repository::open(&fixture.config)?
            .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
            .ok_or("removed Space advertisement missing")?
            .is_active()
    );
    assert!(!respond_has_relay_advertisement(
        &fixture.config,
        (fixture.endpoint_id, peer_id, fixture.committed_space),
        u64::try_from(NOW_MS)?,
    )?);
    assert_eq!(
        fixture
            .handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/private_relay_candidates"),
        Some(&serde_json::json!([]))
    );

    // A replacement Space also publishes immediately at the stepped-back clock.
    let replacement = fixture
        .handle
        .create_owned_space("replacement".to_owned())
        .await?;
    fixture
        .handle
        .set_relay_configuration(enabled_configuration(vec![replacement]))
        .await?;
    let published = Repository::open(&fixture.config)?
        .relay_advertisement(replacement, fixture.endpoint_id)?
        .ok_or("replacement Space advertisement missing")?;
    assert!(published.is_active());
    assert_eq!(published.issued_at_ms(), stepped_back_ms);

    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn clock_behind_retained_partial_batch_still_deactivates_earlier_removed_space() -> TestResult
{
    let state = TempState::new("clock-step-prior-removed")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    let removed = handle.create_owned_space("removed".to_owned()).await?;
    let space_a = handle.create_owned_space("keep-a".to_owned()).await?;
    let space_b = handle.create_owned_space("keep-b".to_owned()).await?;
    let peer_id = EndpointId::from(iroh::SecretKey::from_bytes(&[0x56; 32]).public());
    add_peer(&config, removed, peer_id)?;

    handle
        .set_relay_configuration(enabled_configuration(vec![removed]))
        .await?;
    assert!(
        Repository::open(&config)?
            .relay_advertisement(removed, endpoint_id)?
            .is_some_and(|ad| ad.is_active())
    );

    // Reconfigure away from `removed` to `[space_a, space_b]` and fail mid-batch before
    // durable activity reconciliation completes.
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .set_relay_configuration(enabled_configuration(vec![space_a, space_b]))
            .await
            .is_err()
    );
    assert!(handle.store.relay_publication_pending().await?);

    // Step clock behind the retained partial batch and run maintenance. The retained batch
    // must not replay backdated records, yet `removed` must still be deactivated.
    clock.0.store(NOW_MS - 1_000, Ordering::Relaxed);
    maintenance_tick(&handle).await;

    assert!(handle.store.relay_publication_pending().await?);
    assert!(
        !Repository::open(&config)?
            .relay_advertisement(removed, endpoint_id)?
            .ok_or("removed Space advertisement missing")?
            .is_active()
    );
    assert!(!respond_has_relay_advertisement(
        &config,
        (endpoint_id, peer_id, removed),
        u64::try_from(NOW_MS)?,
    )?);

    runtime.shutdown().await?;
    Ok(())
}
