use std::sync::atomic::Ordering;

use ma2a_store::Repository;

use super::{NOW_MS, TestResult, partial_relay_fixture};

#[tokio::test(start_paused = true)]
async fn near_expiry_partial_recovery_immediately_publishes_fresh_batch() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("near-expiry").await?;
    let retry_at_ms = NOW_MS + 570_000;
    fixture.clock.0.store(retry_at_ms, Ordering::Relaxed);
    fixture
        .handle
        .publish_relay_advertisements(
            fixture.provider.clone(),
            u64::try_from(retry_at_ms + 600_000)?,
        )
        .await?;

    // The successful retry itself must leave both Spaces safely fresh; no
    // later periodic maintenance tick is available to close an expiry gap.
    let repository = Repository::open(&fixture.config)?;
    for space_id in [fixture.committed_space, fixture.missing_space] {
        let advertisement = repository
            .relay_advertisement(space_id, fixture.endpoint_id)?
            .ok_or("served Space has no advertisement after recovery")?;
        assert_eq!(advertisement.sequence(), fixture.committed.sequence() + 1);
        assert_eq!(advertisement.issued_at_ms(), retry_at_ms);
        assert_eq!(advertisement.expires_at_ms(), retry_at_ms + 600_000);
        assert!(advertisement.is_active());
    }
    assert!(!fixture.handle.store.relay_publication_pending().await?);
    fixture.runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn partial_batch_reuse_has_exact_renewal_and_expiry_boundaries() -> TestResult {
    for (offset_ms, renew) in [
        (299_999, false),
        (300_000, true),
        (599_999, true),
        (600_000, true),
    ] {
        let (_state, fixture) = partial_relay_fixture("boundary").await?;
        let now_ms = NOW_MS + offset_ms;
        fixture.clock.0.store(now_ms, Ordering::Relaxed);
        fixture
            .handle
            .publish_relay_advertisements(
                fixture.provider.clone(),
                u64::try_from(now_ms + 600_000)?,
            )
            .await?;
        assert!(!fixture.handle.store.relay_publication_pending().await?);
        let repository = Repository::open(&fixture.config)?;
        for space_id in [fixture.committed_space, fixture.missing_space] {
            let advertisement = repository
                .relay_advertisement(space_id, fixture.endpoint_id)?
                .ok_or("boundary recovery left a Space without an advertisement")?;
            assert_eq!(
                advertisement.sequence(),
                fixture.committed.sequence() + u64::from(renew)
            );
            assert_eq!(
                advertisement.issued_at_ms(),
                if renew { now_ms } else { NOW_MS }
            );
            assert_eq!(
                advertisement.expires_at_ms(),
                if renew {
                    now_ms + 600_000
                } else {
                    NOW_MS + 600_000
                }
            );
        }
        fixture.runtime.shutdown().await?;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn partially_failed_fresh_replacement_replays_its_own_batch() -> TestResult {
    let (_state, fixture) = partial_relay_fixture("replacement-pending").await?;
    let replacement_at_ms = NOW_MS + 360_000;
    fixture.clock.0.store(replacement_at_ms, Ordering::Relaxed);
    fixture.handle.store.fail_relay_publication_after(1).await?;
    assert!(
        fixture
            .handle
            .publish_relay_advertisements(
                fixture.provider.clone(),
                u64::try_from(replacement_at_ms + 600_000)?,
            )
            .await
            .is_err()
    );
    assert!(fixture.handle.store.relay_publication_pending().await?);
    let replacement = Repository::open(&fixture.config)?
        .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
        .ok_or("fresh replacement did not commit its first Space")?;
    assert_eq!(replacement.sequence(), fixture.committed.sequence() + 1);
    assert_ne!(
        replacement.signed_advertisement(),
        fixture.committed.signed_advertisement()
    );
    assert!(
        Repository::open(&fixture.config)?
            .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
            .is_none()
    );

    fixture
        .clock
        .0
        .store(replacement_at_ms + 120_000, Ordering::Relaxed);
    fixture
        .handle
        .publish_relay_advertisements(
            fixture.provider.clone(),
            u64::try_from(replacement_at_ms + 720_000)?,
        )
        .await?;
    assert!(!fixture.handle.store.relay_publication_pending().await?);
    let repository = Repository::open(&fixture.config)?;
    let replayed = repository
        .relay_advertisement(fixture.committed_space, fixture.endpoint_id)?
        .ok_or("replacement first Space disappeared")?;
    let completed = repository
        .relay_advertisement(fixture.missing_space, fixture.endpoint_id)?
        .ok_or("replacement missing Space did not converge")?;
    assert_eq!(
        replayed.signed_advertisement(),
        replacement.signed_advertisement()
    );
    assert_eq!(completed.sequence(), replacement.sequence());
    assert_eq!(completed.issued_at_ms(), replacement_at_ms);
    assert_eq!(completed.expires_at_ms(), replacement_at_ms + 600_000);
    fixture.runtime.shutdown().await?;
    Ok(())
}
