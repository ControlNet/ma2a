//! A clock rollback in one Space must not starve address or relay publication for other Spaces.
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};

use super::relay_lifecycle_test::{
    NOW_MS, TempState, disabled_configuration, enabled_configuration,
};
use crate::{Runtime, RuntimeClock, control_sync::ControlRoundTrigger, error::RuntimeError};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct MutableClock(AtomicI64);

impl RuntimeClock for MutableClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

#[tokio::test(start_paused = true)]
async fn a_clock_rollback_in_one_space_does_not_starve_other_spaces_or_relay_publication()
-> TestResult {
    // Given: two Spaces where `blocked` sorts before `fresh`, and the private relay serves
    // only `fresh`. Both publish at NOW_MS, then `blocked` records an issue time ahead of
    // the later renewal window (as if only `blocked` published before a clock step back).
    let state = TempState::new("publish-rollback-other-space")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let handle = runtime.handle();
    let first = handle.create_owned_space("first".to_owned()).await?;
    let second = handle.create_owned_space("second".to_owned()).await?;
    let (blocked, fresh) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    handle
        .set_relay_configuration(enabled_configuration(vec![fresh]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let repo = Repository::open(&config)?;
    let blocked_initial = repo
        .address_record(blocked, endpoint_id)?
        .ok_or("blocked initial address missing")?;
    let fresh_initial = repo
        .address_record(fresh, endpoint_id)?
        .ok_or("fresh initial address missing")?;
    let relay_initial = repo
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("fresh initial relay advertisement missing")?;
    drop(repo);

    // Move only `blocked`'s stored address issue time ahead of the upcoming clock reading.
    rusqlite::Connection::open(config.database_path())?.execute(
        "UPDATE address_state SET issued_at_ms = ?1, expires_at_ms = ?2
         WHERE space_id = ?3 AND endpoint_id = ?4",
        (
            NOW_MS + 600_000,
            NOW_MS + 1_200_000,
            blocked.as_bytes().as_slice(),
            endpoint_id.as_bytes().as_slice(),
        ),
    )?;
    let before_address_rounds = handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, ControlRoundTrigger::AddressAdvanced))
        .count();

    // When: the clock reaches the 5-minute refresh window for `fresh`, while still behind
    // `blocked`, and periodic maintenance runs.
    let refresh_ms = NOW_MS + 300_100;
    clock.0.store(refresh_ms, Ordering::Relaxed);
    let period = crate::control_actor::control_period(endpoint_id);
    tokio::time::advance(period).await;
    tokio::task::yield_now().await;
    let _status = handle.status().await?;

    // Then: `blocked` stays untouched, while `fresh` publishes both its address and relay
    // renewal, and address follow-up ran.
    let repo = Repository::open(&config)?;
    let blocked_after = repo
        .address_record(blocked, endpoint_id)?
        .ok_or("blocked address missing")?;
    let fresh_after = repo
        .address_record(fresh, endpoint_id)?
        .ok_or("fresh address missing")?;
    let relay_after = repo
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("fresh relay advertisement missing")?;
    drop(repo);
    assert_eq!(blocked_after.sequence(), blocked_initial.sequence());
    assert!(
        fresh_after.sequence() > fresh_initial.sequence(),
        "unaffected Space address record was starved by an earlier Space's clock rollback"
    );
    assert_eq!(fresh_after.issued_at_ms(), refresh_ms);
    assert!(
        relay_after.sequence() > relay_initial.sequence(),
        "relay renewal was starved by an address clock rollback"
    );
    assert_eq!(relay_after.issued_at_ms(), refresh_ms);
    let after_address_rounds = handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, ControlRoundTrigger::AddressAdvanced))
        .count();
    assert!(
        after_address_rounds > before_address_rounds,
        "address publication follow-up did not run after publishing unaffected Space"
    );

    // And when the private relay is disabled in config while an active advertisement remains,
    // `refresh_local_control_publications` still deactivates it despite `blocked`'s rollback.
    runtime.shutdown().await?;
    Repository::open(&config)?.set_relay_configuration(&disabled_configuration())?;
    let restarted =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let _status = restarted.handle().status().await?;
    let disabled = Repository::open(&config)?
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("disabled relay advertisement missing")?;
    assert!(
        !disabled.is_active(),
        "relay deactivation was starved by an address clock rollback"
    );
    restarted.shutdown().await?;
    Ok(())
}
