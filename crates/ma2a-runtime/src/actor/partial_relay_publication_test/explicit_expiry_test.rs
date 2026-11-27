use std::sync::Arc;

use ma2a_core::{SignedPrivateRelayAdvertisementV1, SpaceId};
use ma2a_net::{PrivateRelayProviderConfig, PrivateRelayProviderLocation, PrivateRelayTransport};
use ma2a_store::{Repository, StoreConfig};

use super::{NOW_MS, TempState, TestResult, enabled_configuration};
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
