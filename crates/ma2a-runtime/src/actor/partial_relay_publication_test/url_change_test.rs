use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::{
    ControlArtifactKind, ControlCursorV1, ControlRequestV1, ControlResponseV1, EndpointId,
    SignedPrivateRelayAdvertisementV1, SpaceId,
};
use ma2a_store::{RelayConfiguration, Repository, StoreConfig};

use super::{
    NOW_MS, PublicationClock, TempState, TestResult,
    activity_reconciliation_test::{add_peer, respond_has_relay_advertisement},
    maintenance_tick,
};
use crate::{Runtime, RuntimeClock, control_sync};

fn relay_configuration(spaces: Vec<SpaceId>, url: &str) -> RelayConfiguration {
    let mut config = super::enabled_configuration(spaces);
    config.private_relay_url = Some(url.to_owned());
    config
}

fn exported_relay_url(
    config: &StoreConfig,
    (local_id, remote_id, space_id): (EndpointId, EndpointId, SpaceId),
    now_ms: u64,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
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
    let url = response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .find(|artifact| artifact.kind() == ControlArtifactKind::RELAY_ADVERTISEMENT)
        .map(|artifact| {
            SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(artifact.signed_bytes())
                .map(|signed| signed.advertisement().relay_url().to_string())
        })
        .transpose()?;
    Ok(url)
}

fn persisted_advertisement(
    repository: &Repository,
    space_id: SpaceId,
    endpoint_id: EndpointId,
) -> Result<
    (ma2a_store::PersistedRelayAdvertisement, String),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let row = repository
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("relay advertisement missing")?;
    let url = SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(row.signed_advertisement())?
        .advertisement()
        .relay_url()
        .to_string();
    Ok((row, url))
}

#[tokio::test(start_paused = true)]
async fn url_change_partial_publication_deactivates_stale_url_row_until_retry() -> TestResult {
    let state = TempState::new("url-change-partial")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    let space_a = handle.create_owned_space("url-a".to_owned()).await?;
    let space_b = handle.create_owned_space("url-b".to_owned()).await?;
    let peer_id = EndpointId::from(iroh::SecretKey::from_bytes(&[0x57; 32]).public());
    add_peer(&config, space_a, peer_id)?;
    add_peer(&config, space_b, peer_id)?;

    let url_a = "https://relay-a.example.invalid/";
    let url_b = "https://relay-b.example.invalid/";
    handle
        .set_relay_configuration(relay_configuration(vec![space_a, space_b], url_a))
        .await?;
    let now_u64 = u64::try_from(NOW_MS)?;
    for space_id in [space_a, space_b] {
        assert_eq!(
            exported_relay_url(&config, (endpoint_id, peer_id, space_id), now_u64)?.as_deref(),
            Some(url_a)
        );
    }

    // Reconfigure to URL B for the same two Spaces and fail after the first Space commits.
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .set_relay_configuration(relay_configuration(vec![space_a, space_b], url_b))
            .await
            .is_err()
    );
    assert!(handle.store.relay_publication_pending().await?);

    let repository = Repository::open(&config)?;
    let (_row_first, url_first) = persisted_advertisement(&repository, space_a, endpoint_id)?;
    let (row_second, url_second) = persisted_advertisement(&repository, space_b, endpoint_id)?;
    let (row_first, _) = persisted_advertisement(&repository, space_a, endpoint_id)?;
    let (committed_space, stale_space, stale_row) = match (url_first.as_str(), url_second.as_str())
    {
        (first, second) if first == url_b && second == url_a => (space_a, space_b, row_second),
        (first, second) if first == url_a && second == url_b => (space_b, space_a, row_first),
        other => return Err(format!("unexpected URL pair after partial fault: {other:?}").into()),
    };
    assert!(
        !stale_row.is_active(),
        "stale URL A row remained active after URL change partial failure"
    );
    drop(repository);

    assert_eq!(
        exported_relay_url(&config, (endpoint_id, peer_id, committed_space), now_u64)?.as_deref(),
        Some(url_b)
    );
    assert!(
        !respond_has_relay_advertisement(&config, (endpoint_id, peer_id, stale_space), now_u64)?,
        "control response must not export stale URL A advertisement"
    );
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/private_relay_candidates"),
        Some(&serde_json::json!([{
            "provider_endpoint_id": crate::api::encode_hex(endpoint_id.as_bytes()),
            "relay_url": url_b,
            "covered_space_ids": [crate::api::encode_hex(committed_space.as_bytes())],
            "home_compatible": false,
        }]))
    );

    // On retry, the remaining Space converges to an active URL B row.
    clock.0.store(NOW_MS + 120_000, Ordering::Relaxed);
    maintenance_tick(&handle).await;
    assert!(!handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    for space_id in [space_a, space_b] {
        let (row, url) = persisted_advertisement(&repository, space_id, endpoint_id)?;
        assert!(row.is_active());
        assert_eq!(url, url_b);
        assert_eq!(
            exported_relay_url(
                &config,
                (endpoint_id, peer_id, space_id),
                u64::try_from(NOW_MS + 120_000)?
            )?
            .as_deref(),
            Some(url_b)
        );
    }

    runtime.shutdown().await?;
    Ok(())
}
