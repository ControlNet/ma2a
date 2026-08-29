//! Enrollment behavior for an Endpoint that is already a current member.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ma2a_core::{
    InviteEntropy, MemberCapabilities, RequestId, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_runtime::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, Runtime, RuntimeClock,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

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
            "ma2a-enrollment-existing-{label}-{}-{serial}",
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn already_current_member_is_rejected_without_new_generation() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let candidate_config = StoreConfig::new(&candidate_state.0);
    let owner_bootstrap = Runtime::start(owner_config.clone()).await?;
    let owner_id = owner_bootstrap.handle().status().await?.endpoint_id();
    owner_bootstrap.shutdown().await?;
    let candidate_bootstrap = Runtime::start(candidate_config.clone()).await?;
    let candidate_id = candidate_bootstrap.handle().status().await?.endpoint_id();
    candidate_bootstrap.shutdown().await?;
    let owner_member = member(owner_id, "owner")?;
    let candidate_member = member(candidate_id, "candidate")?;
    let mut repository = Repository::open(&owner_config)?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_000,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![owner_member, candidate_member];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        1_001,
        SpaceManifestMembership::new(members, vec![]),
    ))?;
    drop(repository);
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(2_000))),
    )
    .await?;
    let candidate = Runtime::start(candidate_config).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            created.space_id(),
            100,
            InviteEntropy::from_bytes([0x31; 16], [0x32; 32]),
        )?)
        .await?;
    let before = owner.handle().status().await?;

    // When
    let error = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x33; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await
        .expect_err("an existing member must not create a redundant generation");

    // Then
    assert_eq!(error.code(), EnrollmentErrorCode::CONFLICT);
    assert_eq!(owner.handle().status().await?.revision(), before.revision());
    assert_eq!(
        Repository::open(&owner_config)?
            .load_space_chain(created.space_id())?
            .ok_or("owner chain missing")?
            .latest_generation(),
        1
    );
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
