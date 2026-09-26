//! Deterministic post-commit enrollment failure; no random transport failure is used.
use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::{Repository, StoreConfig, StoreError};

use super::{maintenance::FaultPoint, relay_lifecycle_test::TempState};
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, error::RuntimeError};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn enrollment_completion_failure_reports_durable_membership() -> TestResult {
    let owner_dir = TempState::new("completion-owner")?;
    let candidate_dir = TempState::new("completion-candidate")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let candidate_config = StoreConfig::new(&candidate_dir.0);
    let candidate = Runtime::start(candidate_config.clone()).await?;
    let space = owner
        .handle()
        .create_owned_space("completion".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    candidate.handle().fail_background_once(
        FaultPoint::EnrollmentCompletion,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let result = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x71; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await;
    let endpoint = candidate.handle().status().await?.endpoint_id();
    assert!(
        Repository::open(&candidate_config)?
            .memberships_for(endpoint)?
            .contains(&space)
    );
    let error = result.expect_err("injected completion failure must not return success");
    assert!(
        error.to_string().contains("membership committed"),
        "{error}"
    );
    assert!(error.committed_revision().is_some());
    assert_eq!(error.stage(), Some(crate::EnrollmentStage::ControlLookup));
    // No invitation is submitted again. The normal maintenance tick retries retained work.
    tokio::time::pause();
    let handle = candidate.handle();
    let before = handle.candidate_refresh_attempts();
    tokio::time::advance(std::time::Duration::from_secs(90)).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle.candidate_refresh_attempts() < before + 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "retained enrollment was not retried"
        );
        tokio::task::yield_now().await;
    }
    assert!(handle.status().await?.memberships.contains(&space));
    assert!(
        Repository::open(&candidate_config)?
            .address_record(space, endpoint)?
            .is_some()
    );
    let expected = crate::control_sync::ControlRoundTrigger::Explicit(
        crate::control_sync::ControlRoundScope::peer(owner.handle().status().await?.endpoint_id()),
    );
    assert_eq!(
        handle
            .control_schedules()
            .iter()
            .filter(|trigger| **trigger == expected)
            .count(),
        1
    );
    let before = handle.candidate_refresh_attempts();
    tokio::time::advance(std::time::Duration::from_secs(90)).await;
    while handle.candidate_refresh_attempts() <= before {
        assert!(
            std::time::Instant::now() < deadline,
            "maintenance did not run"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        handle
            .control_schedules()
            .iter()
            .filter(|trigger| **trigger == expected)
            .count(),
        1
    );
    tokio::time::resume();
    owner.shutdown().await?;
    candidate.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn created_space_completion_failure_retains_projection_work() -> TestResult {
    let state = TempState::new("created-space-completion")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    handle.fail_background_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let error = handle
        .create_owned_space("committed".to_owned())
        .await
        .expect_err("completion fault must not claim full success");
    let revision = error
        .committed_revision()
        .ok_or("missing commit revision")?;
    let endpoint = handle.status().await?.endpoint_id();
    let repository = Repository::open(&config)?;
    let memberships = repository.memberships_for(endpoint)?;
    assert_eq!(memberships.len(), 1);
    let space = *memberships.first().ok_or("missing committed Space")?;
    assert_eq!(repository.revision()?, revision);
    let before = handle.candidate_refresh_attempts();
    tokio::time::advance(std::time::Duration::from_secs(90)).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle.candidate_refresh_attempts() < before + 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "membership completion was lost"
        );
        tokio::task::yield_now().await;
    }
    assert!(handle.status().await?.is_ready());
    assert!(
        Repository::open(&config)?
            .address_record(space, endpoint)?
            .is_some()
    );
    runtime.shutdown().await?;
    Ok(())
}
