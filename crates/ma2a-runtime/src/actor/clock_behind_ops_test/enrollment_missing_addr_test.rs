use ma2a_store::Repository;
use tokio::time::timeout;

use super::{
    NOW_MS, PROMPT, TestResult,
    enrollment_test::{has_member, invite, redeem, start_with_clock},
};
use crate::{
    EnrollmentStage,
    actor::maintenance::FaultPoint,
    error::{RuntimeError, RuntimeErrorKind},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_enrollment_without_published_address_answers_retryable_and_keeps_runtime_alive()
-> TestResult {
    let (_owner_dir, owner_config, _owner_clock, owner) =
        start_with_clock("a03-missing-addr-owner", NOW_MS).await?;
    let (_cand_dir, _cand_config, _cand_clock, candidate) =
        start_with_clock("a03-missing-addr-cand", NOW_MS).await?;

    for _ in 0..2 {
        owner.handle().fail_background_once(
            FaultPoint::AddressObservation,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
    let space = owner
        .handle()
        .create_owned_space("no-addr".to_owned())
        .await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    assert!(
        Repository::open(&owner_config)?
            .address_record(space, owner_id)?
            .is_none(),
        "owner must start with no published address record"
    );

    let ticket = invite(&owner, space).await?;
    owner.handle().clear_background_faults();
    for _ in 0..2 {
        owner.handle().fail_background_once(
            FaultPoint::AddressObservation,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
    let error = redeem(&candidate, ticket.clone(), 0xa5)
        .await?
        .expect_err("enrollment without owner address record must return a retryable failure");
    assert_eq!(error.stage(), Some(EnrollmentStage::OwnerRedemption));
    assert!(
        owner.handle().status().await?.is_ready(),
        "owner Runtime must stay alive after missing-address enrollment"
    );

    // Once the owner publishes an address record, retrying with the same RequestId succeeds.
    owner.handle().clear_background_faults();
    owner.handle().publish_address().await?;
    let joined = redeem(&candidate, ticket, 0xa5).await??;
    let candidate_id = candidate.handle().status().await?.endpoint_id();
    assert!(has_member(joined.chain(), candidate_id));
    assert!(
        candidate
            .handle()
            .status()
            .await?
            .memberships
            .contains(&space)
    );

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}
