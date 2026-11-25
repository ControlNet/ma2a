//! A private-relay renewal keeps a valid window when the clock moves between reads.
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};

use super::relay_lifecycle_test::{NOW_MS, TempState, enabled_configuration};
use crate::{Runtime, RuntimeClock, error::RuntimeError};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A clock that moves forward by `step` milliseconds every time it is read.
#[derive(Debug)]
struct SteppingClock {
    now: AtomicI64,
    step: AtomicI64,
}

impl RuntimeClock for SteppingClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        let step = self.step.load(Ordering::Relaxed);
        Ok(self.now.fetch_add(step, Ordering::Relaxed))
    }
}

#[tokio::test(start_paused = true)]
async fn a_relay_renewal_survives_a_suspension_between_clock_reads() -> TestResult {
    // Given: a provider whose advertisement is due for renewal.
    let state = TempState::new("clock-step")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(SteppingClock {
        now: AtomicI64::new(NOW_MS),
        step: AtomicI64::new(0),
    });
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<SteppingClock>::clone(&clock)).await?;
    let handle = runtime.handle();
    let space_id = handle.create_owned_space("clock".to_owned()).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![space_id]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let initial = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("initial advertisement missing")?
        .sequence();

    // When: every clock read lands after a suspension longer than an advertisement's lifetime.
    clock.now.store(NOW_MS + 300_100, Ordering::Relaxed);
    let lifetime = ma2a_core::MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS;
    clock
        .step
        .store(i64::try_from(lifetime + 1)?, Ordering::Relaxed);
    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    tokio::task::yield_now().await;

    // Then: the Runtime still answers, and the renewal signed one consistent window.
    let _status = handle.status().await?;
    let renewed = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("renewed advertisement missing")?;
    assert!(renewed.sequence() > initial, "not renewed");
    assert_eq!(
        renewed.expires_at_ms() - renewed.issued_at_ms(),
        i64::try_from(lifetime)?
    );
    runtime.shutdown().await?;
    Ok(())
}
