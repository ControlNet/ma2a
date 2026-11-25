//! Redemptions waiting on their Space owners are bounded where the actor admits them.
//!
//! The exchanges run on real Iroh I/O, so this uses real time, as `redemption_test` does.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, Runtime};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redemption_past_the_pending_bound_is_refused_at_once() -> TestResult {
    // Given: an invitation whose Space owner has stopped answering.
    let owner_dir = TempState::new("redemption-bound-owner")?;
    let member_dir = TempState::new("redemption-bound-member")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("redemption-bound".to_owned())
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
    let attempt = |byte: u8| -> Result<EnrollmentAttempt, ma2a_core::ProtocolError> {
        Ok(EnrollmentAttempt::new(
            ticket.clone(),
            RequestId::try_from([byte; 16].as_slice())?,
            "member".to_owned(),
        ))
    };

    // When: as many redemptions as are allowed wait on the owner, and one more is made.
    let mut pending = Vec::new();
    for byte in 0..u8::try_from(crate::enrollment_actor::MAX_PENDING_REDEMPTIONS)? {
        pending.push(tokio::spawn(
            member.handle().redeem_enrollment(attempt(byte)?),
        ));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let refused = timeout(PROMPT, member.handle().redeem_enrollment(attempt(0xff)?)).await?;

    // Then: the extra one is refused at once as retryable, and the Runtime keeps answering.
    assert_eq!(
        refused.err().map(|error| error.code()),
        Some(EnrollmentErrorCode::BUSY)
    );
    assert!(pending.iter().all(|redeeming| !redeeming.is_finished()));
    timeout(PROMPT, member.handle().status()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    for redeeming in pending {
        assert!(timeout(PROMPT, redeeming).await??.is_err());
    }
    Ok(())
}
