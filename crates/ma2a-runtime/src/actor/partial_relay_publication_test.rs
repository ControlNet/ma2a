use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};

use super::relay_lifecycle_test::{NOW_MS, TempState, enabled_configuration};
use crate::{Runtime, RuntimeClock, error::RuntimeError};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct PublicationClock(AtomicI64);

impl RuntimeClock for PublicationClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

fn relay_rounds(handle: &super::RuntimeHandle) -> usize {
    handle
        .control_schedules()
        .iter()
        .filter(|trigger| {
            matches!(
                trigger,
                crate::control_sync::ControlRoundTrigger::RelayAdvanced
            )
        })
        .count()
}

async fn maintenance_tick(handle: &super::RuntimeHandle) {
    let before = handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, crate::control_sync::ControlRoundTrigger::Periodic))
        .count();
    let period = crate::control_actor::control_period(handle.status().await.unwrap().endpoint_id());
    tokio::time::advance(period).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, crate::control_sync::ControlRoundTrigger::Periodic))
        .count()
        <= before
    {
        assert!(
            std::time::Instant::now() < deadline,
            "maintenance tick did not finish"
        );
        tokio::task::yield_now().await;
    }
}

struct PartialRelayFixture {
    runtime: Runtime,
    handle: super::RuntimeHandle,
    config: StoreConfig,
    endpoint_id: ma2a_core::EndpointId,
    provider: ma2a_net::PrivateRelayProviderConfig,
    committed: ma2a_store::PersistedRelayAdvertisement,
    committed_space: ma2a_core::SpaceId,
    missing_space: ma2a_core::SpaceId,
    clock: Arc<PublicationClock>,
}

async fn partial_relay_fixture(
    label: &str,
) -> Result<(TempState, PartialRelayFixture), Box<dyn std::error::Error + Send + Sync>> {
    let state = TempState::new(label)?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(PublicationClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::clone(&clock) as Arc<dyn RuntimeClock>)
            .await?;
    let handle = runtime.handle();
    let spaces = [
        handle.create_owned_space("partial-a".to_owned()).await?,
        handle.create_owned_space("partial-b".to_owned()).await?,
    ];
    let configuration = enabled_configuration(spaces.to_vec());
    let provider = ma2a_net::RuntimeRelayConfiguration::try_from(configuration.clone())?
        .private_provider()
        .ok_or("private provider missing")?
        .clone();
    let endpoint_id = handle.status().await?.endpoint_id();
    handle.store.fail_relay_publication_after(1).await?;
    assert!(handle.set_relay_configuration(configuration).await.is_err());
    assert!(handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&config)?;
    let [space_a, space_b] = spaces;
    let records = (
        repository.relay_advertisement(space_a, endpoint_id)?,
        repository.relay_advertisement(space_b, endpoint_id)?,
    );
    let (committed_space, missing_space, committed) = match records {
        (Some(committed), None) => (space_a, space_b, committed),
        (None, Some(committed)) => (space_b, space_a, committed),
        _ => return Err("fault did not stop exactly after one Space".into()),
    };
    assert_eq!(committed.issued_at_ms(), NOW_MS);
    assert_eq!(committed.expires_at_ms(), NOW_MS + 600_000);
    Ok((
        state,
        PartialRelayFixture {
            runtime,
            handle,
            config,
            endpoint_id,
            provider,
            committed,
            committed_space,
            missing_space,
            clock,
        },
    ))
}

#[tokio::test(start_paused = true)]
async fn partial_relay_batch_retries_identical_bytes_without_sequence_or_revision_churn()
-> TestResult {
    let (_state, fixture) = partial_relay_fixture("partial-batch").await?;
    let handle = &fixture.handle;
    handle
        .publish_relay_advertisements(fixture.provider.clone(), u64::try_from(NOW_MS + 600_000)?)
        .await?;
    assert!(!handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&fixture.config)?;
    let replayed = repository
        .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
        .ok_or("first Space disappeared")?;
    let completed = repository
        .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
        .ok_or("second Space did not converge")?;
    assert_eq!(
        replayed.signed_advertisement(),
        fixture.committed.signed_advertisement()
    );
    assert_eq!(replayed.sequence(), fixture.committed.sequence());
    assert_eq!(completed.sequence(), fixture.committed.sequence());
    assert_eq!(completed.issued_at_ms(), fixture.committed.issued_at_ms());
    assert_eq!(completed.expires_at_ms(), fixture.committed.expires_at_ms());
    let revision = repository.revision()?;
    let rounds = relay_rounds(handle);
    drop(repository);
    for _ in 0..3 {
        maintenance_tick(handle).await;
    }
    assert_eq!(Repository::open(&fixture.config)?.revision()?, revision);
    assert_eq!(relay_rounds(handle), rounds);
    assert_eq!(
        Repository::open(&fixture.config)?
            .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
            .ok_or("second Space missing")?
            .sequence(),
        fixture.committed.sequence()
    );
    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn delayed_partial_relay_retry_renews_from_signed_window() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("delayed-batch").await?;
    fixture.clock.0.store(NOW_MS + 360_000, Ordering::Relaxed);
    fixture
        .handle
        .publish_relay_advertisements(fixture.provider.clone(), u64::try_from(NOW_MS + 960_000)?)
        .await?;
    let completed = Repository::open(&fixture.config)?
        .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
        .ok_or("second Space missing")?;
    assert_eq!(completed.sequence(), fixture.committed.sequence());
    assert_eq!(completed.issued_at_ms(), NOW_MS);
    assert_eq!(completed.expires_at_ms(), NOW_MS + 600_000);
    maintenance_tick(&fixture.handle).await;
    let renewed = Repository::open(&fixture.config)?
        .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
        .ok_or("renewed advertisement missing")?;
    assert_eq!(renewed.sequence(), fixture.committed.sequence() + 1);
    assert_eq!(renewed.issued_at_ms(), NOW_MS + 360_000);
    assert_eq!(renewed.expires_at_ms(), NOW_MS + 960_000);
    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn expired_partial_relay_batch_is_replaced_with_new_sequence() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("expired-batch").await?;
    fixture.clock.0.store(NOW_MS + 600_001, Ordering::Relaxed);
    fixture
        .handle
        .publish_relay_advertisements(fixture.provider.clone(), u64::try_from(NOW_MS + 1_200_001)?)
        .await?;
    assert!(!fixture.handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&fixture.config)?;
    for space_id in [fixture.committed_space, fixture.missing_space] {
        let advertisement = repository
            .relay_advertisement(space_id, fixture.endpoint_id)?
            .ok_or("replacement advertisement missing")?;
        assert_eq!(advertisement.sequence(), fixture.committed.sequence() + 1);
        assert_eq!(advertisement.issued_at_ms(), NOW_MS + 600_001);
        assert_eq!(advertisement.expires_at_ms(), NOW_MS + 1_200_001);
    }
    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn clock_rollback_does_not_replay_future_partial_batch() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("rollback-batch").await?;
    fixture.clock.0.store(NOW_MS - 1, Ordering::Relaxed);
    assert!(
        fixture
            .handle
            .publish_relay_advertisements(
                fixture.provider.clone(),
                u64::try_from(NOW_MS + 599_999)?
            )
            .await
            .is_err()
    );
    assert!(fixture.handle.store.relay_publication_pending().await?);
    assert!(
        Repository::open(&fixture.config)?
            .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
            .is_none()
    );
    fixture.clock.0.store(NOW_MS, Ordering::Relaxed);
    fixture
        .handle
        .publish_relay_advertisements(fixture.provider.clone(), u64::try_from(NOW_MS + 600_000)?)
        .await?;
    assert!(!fixture.handle.store.relay_publication_pending().await?);
    fixture.runtime.shutdown().await?;
    Ok(())
}
