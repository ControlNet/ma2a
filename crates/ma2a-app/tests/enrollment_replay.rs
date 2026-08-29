//! End-to-end enrollment replay coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ma2a_core::{InviteEntropy, MemberCapabilities, RequestId, SpaceMemberV1, SpacePolicyV1};
use ma2a_runtime::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, Runtime, RuntimeClock,
};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct TestClock(AtomicI64);

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> Result<i64, ma2a_runtime::RuntimeError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

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
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(3_000))),
    )
    .await?;
    let first = Runtime::start(StoreConfig::new(&first_state.0)).await?;
    let second = Runtime::start(StoreConfig::new(&second_state.0)).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            created.space_id(),
            300_000,
            InviteEntropy::from_bytes([0x51; 16], [0x52; 32]),
        )?)
        .await?;
    let request_id = RequestId::try_from([0x53; 16].as_slice())?;
    let first_attempt = EnrollmentAttempt::new(ticket.clone(), request_id, "first".to_owned());
    let second_attempt = EnrollmentAttempt::new(ticket.clone(), request_id, "second".to_owned());

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
    owner.shutdown().await?;
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(3_000))),
    )
    .await?;
    let before_denials = owner.handle().status().await?;
    let before_generation = Repository::open(&owner_config)?
        .load_space_chain(created.space_id())?
        .ok_or("owner chain missing")?
        .latest_generation();
    let retry = winner.clone().redeem_enrollment(first_attempt).await?;
    assert_eq!(retry.generation(), 1);
    let changed_request = winner
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket.clone(),
            RequestId::try_from([0x54; 16].as_slice())?,
            "changed-request".to_owned(),
        ))
        .await
        .expect_err("changed RequestId retry must conflict");
    assert_eq!(changed_request.code(), EnrollmentErrorCode::CONFLICT);
    let wrong = if first_result.is_ok() {
        second.handle()
    } else {
        first.handle()
    }
    .redeem_enrollment(EnrollmentAttempt::new(
        ticket,
        request_id,
        "wrong".to_owned(),
    ))
    .await
    .expect_err("wrong candidate replay must fail");
    assert_eq!(wrong.code(), EnrollmentErrorCode::CONFLICT);
    assert_eq!(
        owner.handle().status().await?.revision(),
        before_denials.revision()
    );
    assert_eq!(
        Repository::open(&owner_config)?
            .load_space_chain(created.space_id())?
            .ok_or("owner chain missing")?
            .latest_generation(),
        before_generation
    );
    first.shutdown().await?;
    second.shutdown().await?;
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
