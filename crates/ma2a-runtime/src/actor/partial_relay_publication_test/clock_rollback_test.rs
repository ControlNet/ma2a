use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::SpaceId;
use ma2a_store::{Repository, StoreConfig, StoreError};

use super::{
    NOW_MS, PublicationClock, TempState, TestResult, enabled_configuration, maintenance_tick,
    partial_relay_fixture,
};
use crate::{Runtime, RuntimeClock, actor::maintenance::FaultPoint, api, error::RuntimeError};

async fn covered_private_relay_spaces(
    handle: &crate::actor::RuntimeHandle,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let snapshot = handle.snapshot().await?.to_value();
    let candidates = snapshot
        .pointer("/private_relay_candidates")
        .and_then(serde_json::Value::as_array)
        .ok_or("private_relay_candidates missing")?;
    let mut covered = Vec::new();
    for candidate in candidates {
        let spaces = candidate
            .pointer("/covered_space_ids")
            .and_then(serde_json::Value::as_array)
            .ok_or("covered_space_ids missing")?;
        for space in spaces {
            covered.push(
                space
                    .as_str()
                    .ok_or("covered_space_ids entry is not a string")?
                    .to_owned(),
            );
        }
    }
    Ok(covered)
}

#[tokio::test(start_paused = true)]
async fn private_relay_candidates_refresh_after_mid_batch_publication_failure() -> TestResult {
    let state = TempState::new("candidates-mid-batch-fail")?;
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

    handle
        .set_relay_configuration(enabled_configuration(vec![removed]))
        .await?;
    assert_eq!(
        covered_private_relay_spaces(&handle).await?,
        vec![api::encode_hex(removed.as_bytes())]
    );

    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .set_relay_configuration(enabled_configuration(vec![space_a, space_b]))
            .await
            .is_err()
    );
    let repository = Repository::open(&config)?;
    let committed_space: SpaceId = match (
        repository.relay_advertisement(space_a, endpoint_id)?,
        repository.relay_advertisement(space_b, endpoint_id)?,
    ) {
        (Some(_), None) => space_a,
        (None, Some(_)) => space_b,
        _ => return Err("fault did not stop exactly after one replacement Space".into()),
    };
    drop(repository);

    assert_eq!(
        covered_private_relay_spaces(&handle).await?,
        vec![api::encode_hex(committed_space.as_bytes())]
    );

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn private_relay_candidates_refresh_when_clock_behind_returns_after_reconciliation()
-> TestResult {
    let state = TempState::new("candidates-clock-behind-fail")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let removed = handle.create_owned_space("removed".to_owned()).await?;
    let space_a = handle.create_owned_space("keep-a".to_owned()).await?;
    let space_b = handle.create_owned_space("keep-b".to_owned()).await?;

    handle
        .set_relay_configuration(enabled_configuration(vec![removed]))
        .await?;
    let removed_hex = api::encode_hex(removed.as_bytes());
    assert_eq!(
        covered_private_relay_spaces(&handle).await?,
        vec![removed_hex.clone()]
    );

    let next_configuration = enabled_configuration(vec![space_a, space_b]);
    let provider = ma2a_net::RuntimeRelayConfiguration::try_from(next_configuration.clone())?
        .private_provider()
        .ok_or("private provider missing")?
        .clone();
    handle.fail_background_once(
        FaultPoint::RelayPublication,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .set_relay_configuration(next_configuration)
            .await
            .is_err()
    );
    // The follow-up fault kept the actor candidate cache stale with `removed`.
    assert!(
        covered_private_relay_spaces(&handle)
            .await?
            .contains(&removed_hex)
    );

    // Step the clock behind the retained batch and retry publication directly.
    let stepped_back_ms = NOW_MS - 1_000;
    clock.0.store(stepped_back_ms, Ordering::Relaxed);
    assert!(
        handle
            .publish_relay_advertisements(provider, u64::try_from(stepped_back_ms + 600_000)?)
            .await
            .is_err()
    );
    assert!(
        !covered_private_relay_spaces(&handle)
            .await?
            .contains(&removed_hex)
    );

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn future_pending_partial_batch_publishes_eligible_space_on_clock_rollback() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("future-batch-eligible").await?;
    let stepped_back_ms = NOW_MS - 1_000;
    fixture.clock.0.store(stepped_back_ms, Ordering::Relaxed);
    maintenance_tick(&fixture.handle).await;

    assert!(!fixture.handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&fixture.config)?;
    let kept = repository
        .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
        .ok_or("committed Space advertisement missing")?;
    assert_eq!(kept.sequence(), fixture.committed.sequence());
    assert_eq!(
        kept.signed_advertisement(),
        fixture.committed.signed_advertisement()
    );
    assert_eq!(kept.issued_at_ms(), NOW_MS);
    assert!(kept.is_active());

    let published = repository
        .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
        .ok_or("eligible Space did not publish after clock rollback")?;
    assert!(published.is_active());
    assert_eq!(published.sequence(), fixture.committed.sequence() + 1);
    assert_eq!(published.issued_at_ms(), stepped_back_ms);
    assert_eq!(published.expires_at_ms(), stepped_back_ms + 600_000);
    drop(repository);

    assert_eq!(
        covered_private_relay_spaces(&fixture.handle).await?,
        vec![api::encode_hex(fixture.missing_space.as_bytes())]
    );

    fixture
        .clock
        .0
        .store(stepped_back_ms + 500, Ordering::Relaxed);
    maintenance_tick(&fixture.handle).await;
    let repository = Repository::open(&fixture.config)?;
    assert_eq!(
        repository
            .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
            .ok_or("committed Space missing after follow-up tick")?
            .sequence(),
        fixture.committed.sequence()
    );
    assert_eq!(
        repository
            .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
            .ok_or("eligible Space missing after follow-up tick")?
            .sequence(),
        published.sequence()
    );

    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn future_unpublished_batch_does_not_block_spaces_after_clock_rollback() -> TestResult {
    let state = TempState::new("future-unpublished-batch")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    let spaces = [
        handle.create_owned_space("unwritten-a".to_owned()).await?,
        handle.create_owned_space("unwritten-b".to_owned()).await?,
    ];

    handle.store.fail_relay_publication_after(0).await?;
    assert!(
        handle
            .set_relay_configuration(enabled_configuration(spaces.to_vec()))
            .await
            .is_err()
    );
    assert!(handle.store.relay_publication_pending().await?);

    let stepped_back_ms = NOW_MS - 1_000;
    clock.0.store(stepped_back_ms, Ordering::Relaxed);
    maintenance_tick(&handle).await;

    assert!(!handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    for space_id in spaces {
        let published = repository
            .relay_advertisement(space_id, endpoint_id)?
            .ok_or("Space did not publish after discarding unpublished future batch")?;
        assert!(published.is_active());
        assert_eq!(published.issued_at_ms(), stepped_back_ms);
        assert_eq!(published.expires_at_ms(), stepped_back_ms + 600_000);
    }

    runtime.shutdown().await?;
    Ok(())
}
