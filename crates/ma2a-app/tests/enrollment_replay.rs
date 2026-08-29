//! End-to-end enrollment replay and denial coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_tickets::Ticket as _;
use ma2a_core::{
    InviteEntropy, MemberCapabilities, RequestId, SignedInviteTicket, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_runtime::{EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, Runtime};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-enrollment-replay-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn race_exact_retry_and_wrong_candidate_replay_fail_closed() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let first_state = TempState::new("first")?;
    let second_state = TempState::new("second")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let bootstrap = Runtime::start(owner_config.clone()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&owner_config)?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        member(owner_id, "owner")?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);
    let owner = Runtime::start(owner_config.clone()).await?;
    let first = Runtime::start(StoreConfig::new(&first_state.0)).await?;
    let second = Runtime::start(StoreConfig::new(&second_state.0)).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            created.space_id(),
            2_000,
            302_000,
            InviteEntropy::from_bytes([0x51; 16], [0x52; 32]),
        ))
        .await?;
    let request_id = RequestId::try_from([0x53; 16].as_slice())?;
    let first_attempt =
        EnrollmentAttempt::new(ticket.clone(), request_id, "first".to_owned(), 3_000);
    let second_attempt =
        EnrollmentAttempt::new(ticket.clone(), request_id, "second".to_owned(), 3_000);

    // When
    let (first_result, second_result) = tokio::join!(
        first.handle().redeem_enrollment(first_attempt.clone()),
        second.handle().redeem_enrollment(second_attempt),
    );

    // Then
    assert_eq!(
        usize::from(first_result.is_ok()) + usize::from(second_result.is_ok()),
        1
    );
    let winner = if first_result.is_ok() {
        first.handle()
    } else {
        second.handle()
    };
    let retry = winner.redeem_enrollment(first_attempt).await?;
    assert_eq!(retry.generation(), 1);
    let wrong = if first_result.is_ok() {
        second.handle()
    } else {
        first.handle()
    }
    .redeem_enrollment(EnrollmentAttempt::new(
        ticket,
        request_id,
        "wrong".to_owned(),
        3_000,
    ))
    .await
    .expect_err("wrong candidate replay must fail");
    assert_eq!(wrong.code(), EnrollmentErrorCode::CONFLICT);
    first.shutdown().await?;
    second.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one scenario verifies all invitation denial states against the same runtime setup"
)]
async fn expiry_cancellation_cross_space_and_plaintext_secret_are_denied() -> TestResult {
    // Given
    let owner_state = TempState::new("denials-owner")?;
    let candidate_state = TempState::new("denials-candidate")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let bootstrap = Runtime::start(owner_config.clone()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&owner_config)?;
    let first = repository.create_owned_space(&SpaceCreation::new(
        10,
        member(owner_id, "owner")?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let second = repository.create_owned_space(&SpaceCreation::new(
        11,
        member(owner_id, "owner")?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);
    let owner = Runtime::start(owner_config.clone()).await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_state.0)).await?;
    let secret = [0x62; 32];
    let entropy = InviteEntropy::from_bytes([0x61; 16], secret);
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            first.space_id(),
            100,
            200,
            entropy.clone(),
        ))
        .await?;
    let database = fs::read(owner_config.database_path())?;
    assert!(!database.windows(32).any(|window| window == secret));

    // When
    let expired = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket.clone(),
            RequestId::try_from([0x63; 16].as_slice())?,
            "candidate".to_owned(),
            201,
        ))
        .await
        .expect_err("expired invite must fail");
    let cancelled_ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            first.space_id(),
            300,
            400,
            InviteEntropy::from_bytes([0x64; 16], [0x65; 32]),
        ))
        .await?;
    owner
        .handle()
        .cancel_enrollment_invite(cancelled_ticket.invitation_id(), 301)
        .await?;
    let cancelled = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            cancelled_ticket,
            RequestId::try_from([0x66; 16].as_slice())?,
            "candidate".to_owned(),
            302,
        ))
        .await
        .expect_err("cancelled invite must fail");
    let cross = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            second.space_id(),
            500,
            600,
            InviteEntropy::from_bytes([0x67; 16], [0x68; 32]),
        ))
        .await?;
    let mut cross_bytes = cross.encode_bytes();
    let replacement = first
        .space_id()
        .as_bytes()
        .first()
        .ok_or("first Space id is empty")?
        ^ second
            .space_id()
            .as_bytes()
            .first()
            .ok_or("second Space id is empty")?;
    *cross_bytes.get_mut(17).ok_or("ticket Space byte missing")? ^= replacement;
    let cross = SignedInviteTicket::decode_bytes(&cross_bytes)?;
    let cross_error = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            cross,
            RequestId::try_from([0x69; 16].as_slice())?,
            "candidate".to_owned(),
            501,
        ))
        .await
        .expect_err("cross-Space ticket must fail");

    // Then
    assert_eq!(expired.code(), EnrollmentErrorCode::EXPIRED);
    assert_eq!(cancelled.code(), EnrollmentErrorCode::CANCELLED);
    assert_eq!(cross_error.code(), EnrollmentErrorCode::INVALID_TICKET);
    assert_eq!(candidate.handle().status().await?.membership_count(), 0);
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

fn member(endpoint_id: ma2a_core::EndpointId, label: &str) -> TestResultValue<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint_id,
        label.to_owned(),
        MemberCapabilities::new(true, false),
    )?)
}
