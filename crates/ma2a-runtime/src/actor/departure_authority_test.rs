//! A departure sent to the Endpoint that created a Space but no longer holds its authority key
//! is refused, and that Endpoint keeps running.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, SpaceDepartureErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creator_without_the_authority_key_refuses_a_departure_and_keeps_running() -> TestResult {
    // Given: a Space with one member, whose creator has since lost its authority key, as a
    // Runtime that imported the public chain under the same Endpoint would have.
    let owner_dir = TempState::new("departure-keyless-owner")?;
    let member_dir = TempState::new("departure-keyless-member")?;
    let owner_config = StoreConfig::new(&owner_dir.0);
    let owner = Runtime::start(owner_config.clone()).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("departure-keyless".to_owned())
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
            RequestId::try_from([0x71; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;
    rusqlite::Connection::open(owner_config.database_path())?
        .execute("UPDATE spaces SET authority_key_ref = NULL", [])?;
    let owner = Runtime::start(owner_config).await?;

    // When: the member asks to leave.
    let refused = timeout(
        PROMPT,
        member
            .handle()
            .leave_space(space, RequestId::try_from([0x72; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a creator without the authority key cannot remove the member")?;

    // Then: the departure is refused, and the creator still answers.
    assert_eq!(
        refused.code(),
        SpaceDepartureErrorCode::REJECTED,
        "{refused}"
    );
    timeout(PROMPT, owner.handle().status()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}
