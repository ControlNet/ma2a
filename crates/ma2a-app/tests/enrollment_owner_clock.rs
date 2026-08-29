//! Owner-authoritative enrollment time coverage.

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
            "ma2a-enrollment-clock-{label}-{}-{serial}",
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

type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_clock_rejects_expired_ticket_without_state_change() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let bootstrap = Runtime::start(owner_config.clone()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&owner_config)?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        member(owner_id, "owner")?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(201))),
    )
    .await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_state.0)).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            created.space_id(),
            100,
            200,
            InviteEntropy::from_bytes([0x71; 16], [0x72; 32]),
        )?)
        .await?;
    let before = owner.handle().status().await?;
    let generation = Repository::open(&owner_config)?
        .load_space_chain(created.space_id())?
        .ok_or("owner chain missing")?
        .latest_generation();

    // When
    let error = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x73; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await
        .expect_err("expired ticket must fail regardless of candidate input");

    // Then
    assert_eq!(error.code(), EnrollmentErrorCode::EXPIRED);
    assert_eq!(owner.handle().status().await?.revision(), before.revision());
    assert_eq!(
        Repository::open(&owner_config)?
            .load_space_chain(created.space_id())?
            .ok_or("owner chain missing")?
            .latest_generation(),
        generation
    );
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
