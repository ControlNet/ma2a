use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::SpaceId;
use ma2a_store::{Repository, StoreConfig, StoreError};

use super::{NOW_MS, PublicationClock, TempState, TestResult, enabled_configuration};
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
