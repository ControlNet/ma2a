use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};

use super::{MutableClock, NOW_MS, TempState, TestResult, enabled_configuration};
use crate::{Runtime, control_sync::ControlRoundTrigger};

fn push_relay_issued_at(
    config: &StoreConfig,
    (space, endpoint_id): (ma2a_core::SpaceId, ma2a_core::EndpointId),
    issued_at_ms: i64,
) -> TestResult {
    rusqlite::Connection::open(config.database_path())?.execute(
        "UPDATE relay_advertisement_state SET issued_at_ms = ?1, expires_at_ms = ?2
         WHERE space_id = ?3 AND relay_endpoint_id = ?4",
        (
            issued_at_ms,
            issued_at_ms + 600_000,
            space.as_bytes().as_slice(),
            endpoint_id.as_bytes().as_slice(),
        ),
    )?;
    Ok(())
}

fn relay_round_count(handle: &crate::RuntimeHandle) -> usize {
    handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, ControlRoundTrigger::RelayAdvanced))
        .count()
}

#[tokio::test(start_paused = true)]
async fn relay_rollback_does_not_starve_other_spaces_deactivation_or_reconfiguration() -> TestResult
{
    let state = TempState::new("relay-rollback-partial")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let handle = runtime.handle();
    let mut ordered = [
        handle.create_owned_space("space-1".to_owned()).await?,
        handle.create_owned_space("space-2".to_owned()).await?,
        handle.create_owned_space("space-3".to_owned()).await?,
    ];
    ordered.sort();
    let [blocked, fresh, removed] = ordered;

    handle
        .set_relay_configuration(enabled_configuration(vec![blocked, fresh, removed]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let repo = Repository::open(&config)?;
    let blocked_initial = repo
        .relay_advertisement(blocked, endpoint_id)?
        .ok_or("blocked initial relay ad missing")?;
    let fresh_initial = repo
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("fresh initial relay ad missing")?;
    drop(repo);

    push_relay_issued_at(&config, (blocked, endpoint_id), NOW_MS + 600_000)?;

    let refresh_ms = NOW_MS + 300_100;
    clock.0.store(refresh_ms, Ordering::Relaxed);
    let before_relay_rounds = relay_round_count(&handle);
    let reconfigured = handle
        .set_relay_configuration(enabled_configuration(vec![blocked, fresh]))
        .await;
    assert!(
        reconfigured.is_err(),
        "set_relay_configuration must still report ClockBehind while blocked is served"
    );

    let repo = Repository::open(&config)?;
    let blocked_after = repo
        .relay_advertisement(blocked, endpoint_id)?
        .ok_or("blocked relay ad missing")?;
    let fresh_after = repo
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("fresh relay ad missing")?;
    let removed_after = repo
        .relay_advertisement(removed, endpoint_id)?
        .ok_or("removed relay ad missing")?;
    drop(repo);
    assert_eq!(blocked_after.sequence(), blocked_initial.sequence());
    assert!(fresh_after.sequence() > fresh_initial.sequence());
    assert_eq!(fresh_after.issued_at_ms(), refresh_ms);
    assert!(!removed_after.is_active());
    assert!(relay_round_count(&handle) > before_relay_rounds);

    push_relay_issued_at(&config, (blocked, endpoint_id), NOW_MS + 900_000)?;
    let second_refresh_ms = refresh_ms + 300_100;
    clock.0.store(second_refresh_ms, Ordering::Relaxed);
    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    tokio::task::yield_now().await;
    let _status = handle.status().await?;

    let fresh_second = Repository::open(&config)?
        .relay_advertisement(fresh, endpoint_id)?
        .ok_or("fresh second relay ad missing")?;
    assert!(fresh_second.sequence() > fresh_after.sequence());
    assert_eq!(fresh_second.issued_at_ms(), second_refresh_ms);

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn dropping_a_clock_behind_space_clears_the_cached_relay_issue_time() -> TestResult {
    // Given: a served Space published at NOW_MS + 300_100, then the clock steps back to
    // NOW_MS + 200_000 (behind `published_relay.issued_at_ms`).
    let state = TempState::new("relay-rollback-reconfigure")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS + 300_100)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    let handle = runtime.handle();
    let old_space = handle.create_owned_space("old".to_owned()).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![old_space]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();

    // When: clock steps back, a new Space is created, and relay is reconfigured to serve only it.
    clock.0.store(NOW_MS + 200_000, Ordering::Relaxed);
    let fresh_space = handle.create_owned_space("fresh".to_owned()).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![fresh_space]))
        .await?;

    // Then: the new Space publishes immediately and the dropped Space is deactivated.
    let repo = Repository::open(&config)?;
    let fresh_ad = repo
        .relay_advertisement(fresh_space, endpoint_id)?
        .ok_or("fresh relay ad missing")?;
    let old_deactivated = repo
        .relay_advertisement(old_space, endpoint_id)?
        .ok_or("old deactivated ad missing")?;
    assert!(fresh_ad.is_active());
    assert_eq!(fresh_ad.issued_at_ms(), NOW_MS + 200_000);
    assert!(!old_deactivated.is_active());

    runtime.shutdown().await?;
    Ok(())
}
