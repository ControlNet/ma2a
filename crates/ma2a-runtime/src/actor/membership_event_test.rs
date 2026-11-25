//! An owner that enrolls a member announces the membership change, as every other change does.
use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, state::RuntimeEvent};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_announces_the_member_it_enrolled() -> TestResult {
    // Given: an owner with an invitation, watched from the moment before redemption.
    let owner_dir = TempState::new("membership-event-owner")?;
    let candidate_dir = TempState::new("membership-event-candidate")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("membership-event".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    let before = owner.handle().status().await?.revision;
    let mut events = owner.handle().subscribe();

    // When
    candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x81; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await?;

    // Then: the owner announced a membership change past the revision it had before.
    let mut announced = false;
    while let Ok(event) = events.try_recv() {
        announced |= event.revision() > before
            && event == RuntimeEvent::memberships_changed(event.revision());
    }
    assert!(announced, "the owner did not announce the enrollment");
    owner.shutdown().await?;
    candidate.shutdown().await?;
    Ok(())
}
