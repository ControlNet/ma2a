use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use ma2a_core::{EndpointId, InviteEntropy, RequestId, SpaceChain};
use ma2a_store::{Repository, StoreConfig};
use tokio::time::timeout;

use super::{MutableClock, NOW_MS, PROMPT, TempState, TestResult};
use crate::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, EnrollmentStage, Runtime,
    control_sync::{ControlRoundScope, ControlRoundTrigger},
};

async fn start_with_clock(
    label: &str,
    now_ms: i64,
) -> Result<
    (TempState, StoreConfig, Arc<MutableClock>, Runtime),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let dir = TempState::new(label)?;
    let config = StoreConfig::new(&dir.0);
    let clock = Arc::new(MutableClock(AtomicI64::new(now_ms)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<MutableClock>::clone(&clock)).await?;
    Ok((dir, config, clock, runtime))
}

async fn invite(
    owner: &Runtime,
    space: ma2a_core::SpaceId,
) -> Result<ma2a_core::SignedInviteTicket, Box<dyn std::error::Error + Send + Sync>> {
    Ok(owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?)
}

async fn redeem(
    candidate: &Runtime,
    ticket: ma2a_core::SignedInviteTicket,
    request_byte: u8,
) -> Result<
    Result<crate::EstablishedEnrollment, crate::EnrollmentError>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let request_id = RequestId::try_from([request_byte; 16].as_slice())?;
    Ok(timeout(
        PROMPT,
        candidate.handle().redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            request_id,
            "candidate".to_owned(),
        )),
    )
    .await?)
}

fn has_member(chain: &SpaceChain, id: EndpointId) -> bool {
    chain.members().iter().any(|m| m.endpoint_id() == id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_enrollment_succeeds_while_unrelated_space_is_clock_behind() -> TestResult {
    let (_owner_dir, owner_config, owner_clock, owner) =
        start_with_clock("c05-owner-behind-owner", NOW_MS).await?;
    let (_cand_dir, _cand_config, _cand_clock, candidate) =
        start_with_clock("c05-owner-behind-cand", NOW_MS).await?;
    let space = owner
        .handle()
        .create_owned_space("target".to_owned())
        .await?;
    owner_clock.0.store(NOW_MS + 30_000, Ordering::Relaxed);
    let unrelated = owner
        .handle()
        .create_owned_space("unrelated".to_owned())
        .await?;
    owner_clock.0.store(NOW_MS, Ordering::Relaxed);
    let owner_id = owner.handle().status().await?.endpoint_id();
    let candidate_id = candidate.handle().status().await?.endpoint_id();
    let initial_unrelated_seq = Repository::open(&owner_config)?
        .address_record(unrelated, owner_id)?
        .ok_or("unrelated initial address missing")?
        .sequence();
    let joined = redeem(&candidate, invite(&owner, space).await?, 0xa1).await??;

    assert!(has_member(joined.chain(), candidate_id));
    let memberships = candidate.handle().status().await?.memberships;
    assert!(memberships.contains(&space));
    let owner_chain = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    assert!(has_member(&owner_chain, candidate_id));

    owner_clock.0.store(NOW_MS + 330_100, Ordering::Relaxed);
    owner.handle().publish_address().await?;
    let after_unrelated = Repository::open(&owner_config)?
        .address_record(unrelated, owner_id)?
        .ok_or("unrelated recovered address missing")?;
    assert!(after_unrelated.sequence() > initial_unrelated_seq);

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn candidate_enrollment_succeeds_while_unrelated_space_is_clock_behind() -> TestResult {
    let (_owner_dir, _owner_config, _owner_clock, owner) =
        start_with_clock("c05-cand-behind-owner", NOW_MS).await?;
    let (_cand_dir, cand_config, cand_clock, candidate) =
        start_with_clock("c05-cand-behind-cand", NOW_MS + 30_000).await?;
    let unrelated = candidate
        .handle()
        .create_owned_space("cand-unrelated".to_owned())
        .await?;
    cand_clock.0.store(NOW_MS, Ordering::Relaxed);
    let owner_id = owner.handle().status().await?.endpoint_id();
    let candidate_id = candidate.handle().status().await?.endpoint_id();
    let initial_unrelated_seq = Repository::open(&cand_config)?
        .address_record(unrelated, candidate_id)?
        .ok_or("candidate unrelated address missing")?
        .sequence();
    let space = owner
        .handle()
        .create_owned_space("target".to_owned())
        .await?;
    let joined = redeem(&candidate, invite(&owner, space).await?, 0xa2).await??;

    assert!(has_member(joined.chain(), candidate_id));
    let memberships = candidate.handle().status().await?.memberships;
    assert!(memberships.contains(&space));
    let expected_trigger = ControlRoundTrigger::Explicit(ControlRoundScope::peer(owner_id));
    let schedules = candidate.handle().control_schedules();
    assert!(schedules.contains(&expected_trigger));
    timeout(PROMPT, candidate.handle().sync_control()).await??;
    let synced = candidate.handle().control_sync_status(owner_id).await?;
    assert!(synced.into_value());

    cand_clock.0.store(NOW_MS + 330_100, Ordering::Relaxed);
    candidate.handle().publish_address().await?;
    let after_unrelated = Repository::open(&cand_config)?
        .address_record(unrelated, candidate_id)?
        .ok_or("candidate unrelated recovered address missing")?;
    assert!(after_unrelated.sequence() > initial_unrelated_seq);

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_for_requested_space_with_future_owner_address_is_still_rejected() -> TestResult {
    let (_owner_dir, _owner_config, _owner_clock, owner) =
        start_with_clock("c05-future-owner", NOW_MS + 30_000).await?;
    let (_cand_dir, _cand_config, _cand_clock, candidate) =
        start_with_clock("c05-future-cand", NOW_MS).await?;
    let space = owner
        .handle()
        .create_owned_space("future".to_owned())
        .await?;
    let error = redeem(&candidate, invite(&owner, space).await?, 0xa3)
        .await?
        .expect_err("future owner address for requested Space must be rejected");
    assert_eq!(error.code(), EnrollmentErrorCode::INVALID_TICKET);
    assert_eq!(error.stage(), Some(EnrollmentStage::BootstrapValidation));
    let memberships = candidate.handle().status().await?.memberships;
    assert!(!memberships.contains(&space));

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_retry_enrollment_succeeds_while_unrelated_space_is_clock_behind() -> TestResult {
    let (_owner_dir, _owner_config, owner_clock, owner) =
        start_with_clock("c05-retry-owner", NOW_MS).await?;
    let space = owner
        .handle()
        .create_owned_space("retry".to_owned())
        .await?;
    owner_clock.0.store(NOW_MS + 30_000, Ordering::Relaxed);
    let _unrelated = owner
        .handle()
        .create_owned_space("unrelated".to_owned())
        .await?;
    owner_clock.0.store(NOW_MS, Ordering::Relaxed);
    let ticket = invite(&owner, space).await?;
    let attempt = EnrollmentAttempt::new(
        ticket,
        RequestId::try_from([0xa4; 16].as_slice())?,
        "retry-candidate".to_owned(),
    );
    let raw_request = crate::enrollment::encode_attempt(&attempt)?;
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let endpoint =
        ma2a_net::RuntimeEndpoint::bind(ma2a_net::EndpointSecret::generate(), tx, None).await?;

    let (first_status, first_pages) = timeout(
        PROMPT,
        endpoint.exchange_redemption(attempt.owner_addr(), raw_request.clone()),
    )
    .await??;
    let (retry_status, retry_pages) = timeout(
        PROMPT,
        endpoint.exchange_redemption(attempt.owner_addr(), raw_request),
    )
    .await??;

    assert_eq!(first_status, 0);
    assert_eq!(retry_status, 0);
    let first_bootstrap = ma2a_core::EnrollmentBootstrap::decode_frames(&first_pages)?;
    let retry_bootstrap = ma2a_core::EnrollmentBootstrap::decode_frames(&retry_pages)?;
    assert_eq!(first_bootstrap.chain(), retry_bootstrap.chain());

    endpoint.shutdown().await?;
    timeout(PROMPT, owner.shutdown()).await??;
    Ok(())
}
