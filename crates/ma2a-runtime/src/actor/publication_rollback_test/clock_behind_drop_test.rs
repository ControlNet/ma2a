use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use ma2a_core::RequestId;
use ma2a_store::{Repository, StoreConfig};

use super::empty_effective_test::enroll_peer_in_space;
use super::relay_rollback_test::push_relay_issued_at;
use super::{MutableClock, NOW_MS, TempState, TestResult, enabled_configuration};
use crate::Runtime;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_clock_behind_space_deactivates_it_without_resigning_healthy_space() -> TestResult
{
    let owner_state = TempState::new("clock-behind-revoke-owner")?;
    let provider_state = TempState::new("clock-behind-revoke-provider")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let provider_config = StoreConfig::new(&provider_state.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let owner = Runtime::start_with_clock(owner_config, Arc::<MutableClock>::clone(&clock)).await?;
    let provider =
        Runtime::start_with_clock(provider_config.clone(), Arc::<MutableClock>::clone(&clock))
            .await?;
    let owner_handle = owner.handle();
    let provider_handle = provider.handle();
    let provider_id = provider_handle.status().await?.endpoint_id();

    let blocked = owner_handle
        .create_owned_space("blocked".to_owned())
        .await?;
    enroll_peer_in_space((&owner_handle, &provider_handle), blocked, 0xa3).await?;
    let fresh = provider_handle
        .create_owned_space("fresh".to_owned())
        .await?;

    provider_handle
        .set_relay_configuration(enabled_configuration(vec![blocked, fresh]))
        .await?;
    push_relay_issued_at(&provider_config, (blocked, provider_id), NOW_MS + 600_000)?;

    let refresh_ms = NOW_MS + 300_100;
    clock.0.store(refresh_ms, Ordering::Relaxed);
    provider_handle
        .set_relay_configuration(enabled_configuration(vec![blocked, fresh]))
        .await?;
    let repo = Repository::open(&provider_config)?;
    assert!(
        repo.relay_advertisement(blocked, provider_id)?
            .ok_or("blocked ad missing")?
            .is_active()
    );
    let fresh_current_seq = repo
        .relay_advertisement(fresh, provider_id)?
        .ok_or("fresh ad missing")?
        .sequence();
    drop(repo);

    // Within `fresh`'s renewal window, leave `blocked` so `blocked` drops out of the
    // effective set without triggering `set_relay_configuration`.
    clock.0.store(refresh_ms + 100, Ordering::Relaxed);
    tokio::time::timeout(
        Duration::from_secs(10),
        provider_handle.leave_space(blocked, RequestId::try_from([0xa5; 16].as_slice())?),
    )
    .await??;

    let repo = Repository::open(&provider_config)?;
    assert!(
        !repo
            .relay_advertisement(blocked, provider_id)?
            .ok_or("revoked blocked ad missing")?
            .is_active()
    );
    let fresh_after = repo
        .relay_advertisement(fresh, provider_id)?
        .ok_or("fresh ad missing after revocation")?;
    assert!(fresh_after.is_active());
    assert_eq!(fresh_after.sequence(), fresh_current_seq);
    drop(repo);

    let snapshot = provider_handle.snapshot().await?.to_value();
    let candidates = snapshot
        .pointer("/private_relay_candidates")
        .and_then(serde_json::Value::as_array)
        .ok_or("private_relay_candidates missing")?;
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates
            .first()
            .and_then(|candidate| candidate.pointer("/covered_space_ids")),
        Some(&serde_json::json!([crate::api::encode_hex(
            fresh.as_bytes()
        )]))
    );

    tokio::time::timeout(Duration::from_secs(10), provider.shutdown()).await??;
    let restarted =
        Runtime::start_with_clock(provider_config.clone(), Arc::<MutableClock>::clone(&clock))
            .await?;
    let _status = restarted.handle().status().await?;
    let repo = Repository::open(&provider_config)?;
    assert!(
        !repo
            .relay_advertisement(blocked, provider_id)?
            .ok_or("revoked blocked ad missing after restart")?
            .is_active()
    );
    assert!(
        repo.relay_advertisement(fresh, provider_id)?
            .ok_or("fresh ad missing after restart")?
            .is_active()
    );

    tokio::time::timeout(Duration::from_secs(10), owner.shutdown()).await??;
    tokio::time::timeout(Duration::from_secs(10), restarted.shutdown()).await??;
    Ok(())
}
