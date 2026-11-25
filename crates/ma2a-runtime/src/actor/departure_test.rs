//! A departure waits on the Space authority without holding the Runtime actor.
//!
//! The exchange runs on real Iroh I/O, so this uses real time: a paused clock would
//! jump straight to the exchange deadline and end the departure before anything waited.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, SpaceDepartureErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_departure_leaves_the_runtime_responsive() -> TestResult {
    // Given: a member whose Space authority has stopped answering.
    let owner_dir = TempState::new("departure-owner")?;
    let member_dir = TempState::new("departure-member")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("departure".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x61; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;

    // When: the member asks to leave and the exchange is still waiting on the authority.
    let first = RequestId::try_from([0x62; 16].as_slice())?;
    let handle = member.handle();
    let leaving = tokio::spawn(async move { handle.leave_space(space, first).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: the Runtime keeps answering, and refuses a second departure of the same Space.
    let status = timeout(PROMPT, member.handle().status()).await??;
    assert!(
        !leaving.is_finished(),
        "the departure must still be pending"
    );
    assert!(status.memberships.contains(&space));
    let second = timeout(
        PROMPT,
        member
            .handle()
            .leave_space(space, RequestId::try_from([0x63; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a second departure of the same Space must be refused")?;
    assert_eq!(
        second.code(),
        SpaceDepartureErrorCode::IN_FLIGHT,
        "{second}"
    );

    // Stopping the Runtime ends the exchange with it instead of waiting it out.
    timeout(PROMPT, member.shutdown()).await??;
    let abandoned = timeout(PROMPT, leaving)
        .await??
        .err()
        .ok_or("a departure cut short by shutdown must not report success")?;
    assert_eq!(
        abandoned.code(),
        SpaceDepartureErrorCode::INTERNAL,
        "{abandoned}"
    );
    Ok(())
}
