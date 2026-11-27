use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::{SignedPrivateRelayAdvertisementV1, SpaceId};
use ma2a_net::{PrivateRelayProviderConfig, PrivateRelayProviderLocation, PrivateRelayTransport};
use ma2a_store::{Repository, StoreConfig};

use super::{
    NOW_MS, PublicationClock, TempState, TestResult, enabled_configuration, maintenance_tick,
};
use crate::{Runtime, RuntimeClock, error::RuntimeError};

#[derive(Debug)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(NOW_MS)
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_publication_honors_shorter_expiry_and_refuses_invalid_windows() -> TestResult {
    let state = TempState::new("explicit-expiry")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let handle = runtime.handle();
    let space_id = handle.create_owned_space("explicit".to_owned()).await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let relay_config = enabled_configuration(vec![space_id]);
    let provider = ma2a_net::RuntimeRelayConfiguration::try_from(relay_config)?
        .private_provider()
        .ok_or("private provider missing")?
        .clone();

    let now_ms = u64::try_from(NOW_MS)?;
    handle
        .publish_relay_advertisements(provider.clone(), now_ms + 600_000)
        .await?;
    let initial = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("initial advertisement missing")?;
    assert_eq!(initial.expires_at_ms(), NOW_MS + 600_000);

    // Request a shorter expiry at the exact same URL while the ten-minute record is still current.
    handle
        .publish_relay_advertisements(provider.clone(), now_ms + 100)
        .await?;
    let shortened = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("shortened advertisement missing")?;
    let decoded =
        SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(shortened.signed_advertisement())?;
    assert!(shortened.sequence() > initial.sequence());
    assert_eq!(shortened.expires_at_ms(), NOW_MS + 100);
    assert_eq!(decoded.advertisement().expires_at_ms(), now_ms + 100);

    // Invalid expiry windows must be refused even when all Spaces were previously published.
    assert!(
        handle
            .publish_relay_advertisements(provider.clone(), now_ms)
            .await
            .is_err()
    );
    assert!(
        handle
            .publish_relay_advertisements(provider.clone(), now_ms - 1)
            .await
            .is_err()
    );
    assert!(
        handle
            .publish_relay_advertisements(provider, now_ms + 600_001)
            .await
            .is_err()
    );

    // Invalid expiry must also be refused when no effective Space matches.
    let unjoined_provider = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            "127.0.0.1:0".parse()?,
            "https://relay.example.invalid/".parse()?,
        ),
        vec![SpaceId::try_from([0xdd; 32].as_slice())?],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    assert!(
        handle
            .publish_relay_advertisements(unjoined_provider, now_ms)
            .await
            .is_err()
    );

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn explicit_retry_with_longer_expiry_replaces_pending_partial_batch() -> TestResult {
    let state = TempState::new("explicit-longer-pending")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let handle = runtime.handle();
    let spaces = [
        handle.create_owned_space("explicit-a".to_owned()).await?,
        handle.create_owned_space("explicit-b".to_owned()).await?,
    ];
    let endpoint_id = handle.status().await?.endpoint_id();
    let provider =
        ma2a_net::RuntimeRelayConfiguration::try_from(enabled_configuration(spaces.to_vec()))?
            .private_provider()
            .ok_or("private provider missing")?
            .clone();
    let now_ms = u64::try_from(NOW_MS)?;

    // Fail after writing one Space with a six-minute expiry so the batch remains pending.
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .publish_relay_advertisements(provider.clone(), now_ms + 360_000)
            .await
            .is_err()
    );
    assert!(handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    let ((Some(committed), None) | (None, Some(committed))) = (
        repository.relay_advertisement(spaces[0], endpoint_id)?,
        repository.relay_advertisement(spaces[1], endpoint_id)?,
    ) else {
        return Err("fault did not stop exactly after one Space".into());
    };
    assert_eq!(committed.expires_at_ms(), NOW_MS + 360_000);
    drop(repository);

    // An explicit request for a longer expiry must replace the pending batch rather than
    // silently returning the shorter six-minute expiry.
    handle
        .publish_relay_advertisements(provider.clone(), now_ms + 480_000)
        .await?;
    assert!(!handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    for space_id in spaces {
        let published = repository
            .relay_advertisement(space_id, endpoint_id)?
            .ok_or("longer-expiry advertisement missing")?;
        let decoded = SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(
            published.signed_advertisement(),
        )?;
        assert_eq!(published.sequence(), committed.sequence() + 1);
        assert_eq!(published.expires_at_ms(), NOW_MS + 480_000);
        assert_eq!(decoded.advertisement().expires_at_ms(), now_ms + 480_000);
    }
    drop(repository);

    // An explicit retry with an equal expiry reuses the pending batch verbatim.
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .publish_relay_advertisements(provider.clone(), now_ms + 420_000)
            .await
            .is_err()
    );
    assert!(handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    let (partial_space, partial_record) = match (
        repository.relay_advertisement(spaces[0], endpoint_id)?,
        repository.relay_advertisement(spaces[1], endpoint_id)?,
    ) {
        (Some(first), Some(second)) if first.expires_at_ms() == NOW_MS + 420_000 => {
            assert_eq!(second.expires_at_ms(), NOW_MS + 480_000);
            (spaces[0], first)
        }
        (Some(first), Some(second)) if second.expires_at_ms() == NOW_MS + 420_000 => {
            assert_eq!(first.expires_at_ms(), NOW_MS + 480_000);
            (spaces[1], second)
        }
        _ => return Err("equal-expiry fault did not stop after one Space".into()),
    };
    drop(repository);

    handle
        .publish_relay_advertisements(provider, now_ms + 420_000)
        .await?;
    assert!(!handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    let replayed = repository
        .relay_advertisement(partial_space, endpoint_id)?
        .ok_or("replayed advertisement missing")?;
    let other_space = if partial_space == spaces[0] {
        spaces[1]
    } else {
        spaces[0]
    };
    let completed = repository
        .relay_advertisement(other_space, endpoint_id)?
        .ok_or("completed advertisement missing")?;
    assert_eq!(
        replayed.signed_advertisement(),
        partial_record.signed_advertisement()
    );
    assert_eq!(completed.sequence(), partial_record.sequence());
    assert_eq!(completed.expires_at_ms(), NOW_MS + 420_000);

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failed_explicit_shortening_does_not_leave_maintenance_skipping_expired_space() -> TestResult
{
    let state = TempState::new("explicit-shorten-fail")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let spaces = [
        handle.create_owned_space("shorten-a".to_owned()).await?,
        handle.create_owned_space("shorten-b".to_owned()).await?,
    ];
    let endpoint_id = handle.status().await?.endpoint_id();
    let relay_config = enabled_configuration(spaces.to_vec());
    let provider = ma2a_net::RuntimeRelayConfiguration::try_from(relay_config.clone())?
        .private_provider()
        .ok_or("private provider missing")?
        .clone();
    handle.set_relay_configuration(relay_config).await?;
    let now_ms = u64::try_from(NOW_MS)?;

    // Shorten the first Space to 100 ms and fail before writing the second Space.
    handle.store.fail_relay_publication_after(1).await?;
    assert!(
        handle
            .publish_relay_advertisements(provider, now_ms + 100)
            .await
            .is_err()
    );
    let repository = Repository::open(&config)?;
    let shortened_space = match (
        repository.relay_advertisement(spaces[0], endpoint_id)?,
        repository.relay_advertisement(spaces[1], endpoint_id)?,
    ) {
        (Some(first), Some(second))
            if first.expires_at_ms() == NOW_MS + 100
                && second.expires_at_ms() == NOW_MS + 600_000 =>
        {
            spaces[0]
        }
        (Some(first), Some(second))
            if second.expires_at_ms() == NOW_MS + 100
                && first.expires_at_ms() == NOW_MS + 600_000 =>
        {
            spaces[1]
        }
        _ => return Err("shortening fault did not leave one shortened Space".into()),
    };
    drop(repository);

    // Advance past the shortened record's expiry and run a maintenance pass.
    let after_expiry_ms = NOW_MS + 200;
    clock.0.store(after_expiry_ms, Ordering::Relaxed);
    maintenance_tick(&handle).await;

    let repository = Repository::open(&config)?;
    let restored = repository
        .relay_advertisement(shortened_space, endpoint_id)?
        .ok_or("shortened Space advertisement missing")?;
    assert!(restored.is_active());
    assert_eq!(restored.issued_at_ms(), after_expiry_ms);
    assert_eq!(restored.expires_at_ms(), after_expiry_ms + 600_000);

    runtime.shutdown().await?;
    Ok(())
}
