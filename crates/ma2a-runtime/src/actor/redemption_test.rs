//! A redemption waits on the Space owner without holding the Runtime actor.
//!
//! The exchange runs on real Iroh I/O, so this uses real time: a paused clock would
//! jump straight to the exchange deadline and end the redemption before anything waited.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_redemption_leaves_the_runtime_responsive() -> TestResult {
    // Given: an invitation whose Space owner has stopped answering.
    let owner_dir = TempState::new("redemption-owner")?;
    let member_dir = TempState::new("redemption-member")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("redemption".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    owner.shutdown().await?;

    // When: the member redeems it and the exchange is still waiting on the owner.
    let attempt = EnrollmentAttempt::new(
        ticket,
        RequestId::try_from([0x71; 16].as_slice())?,
        "member".to_owned(),
    );
    let redeeming = tokio::spawn(member.handle().redeem_enrollment(attempt));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: the Runtime keeps answering while the redemption is pending.
    let status = timeout(PROMPT, member.handle().status()).await??;
    assert!(
        !redeeming.is_finished(),
        "the redemption must still be pending"
    );
    assert!(!status.memberships.contains(&space));

    // Stopping the Runtime ends the exchange with it instead of waiting it out.
    timeout(PROMPT, member.shutdown()).await??;
    let abandoned = timeout(PROMPT, redeeming).await??;
    assert!(
        abandoned.is_err(),
        "a redemption cut short by shutdown must not report success"
    );
    Ok(())
}
