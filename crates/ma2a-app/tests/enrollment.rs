//! End-to-end enrollment chain establishment coverage.

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
    EnrollmentPage, InviteEntropy, MemberCapabilities, RequestId, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1, validate_enrollment_pages,
};
use ma2a_runtime::{EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

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
            "ma2a-enrollment-{label}-{}-{serial}",
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
async fn generation_57_invite_establishes_only_after_contiguous_generation_58() -> TestResult {
    // Given
    let owner_state = TempState::new("owner-57")?;
    let candidate_state = TempState::new("candidate-57")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let candidate_config = StoreConfig::new(&candidate_state.0);
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(3_000))),
    )
    .await?;
    let owner_status = owner.handle().status().await?;
    owner.shutdown().await?;
    let owner_member = member(owner_status.endpoint_id(), "owner")?;
    let mut repository = Repository::open(&owner_config)?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1_700_000_000_000,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let revoked = member(endpoint(0x42)?, "revoked")?;
    let mut first_members = vec![owner_member.clone(), revoked.clone()];
    first_members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        1_700_000_000_001,
        SpaceManifestMembership::new(first_members, vec![]),
    ))?;
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        1_700_000_000_002,
        SpaceManifestMembership::new(
            vec![owner_member.clone()],
            vec![SpaceRevocationV1::new(revoked.endpoint_id())],
        ),
    ))?;
    for generation in 3..=57 {
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            created.space_id(),
            1_700_000_000_000 + generation,
            SpaceManifestMembership::new(
                vec![owner_member.clone()],
                vec![SpaceRevocationV1::new(revoked.endpoint_id())],
            ),
        ))?;
    }
    drop(repository);
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::new(TestClock(AtomicI64::new(3_000))),
    )
    .await?;
    let candidate = Runtime::start(candidate_config.clone()).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            created.space_id(),
            300_000,
            InviteEntropy::from_bytes([0x11; 16], [0x22; 32]),
        )?)
        .await?;

    // When
    let established = candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x33; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await?;

    // Then
    assert_eq!(established.generation(), 58);
    assert_eq!(candidate.handle().status().await?.membership_count(), 1);
    assert!(
        candidate
            .handle()
            .status()
            .await?
            .normal_protocols_eligible()
    );
    let chain = Repository::open(&candidate_config)?
        .load_space_chain(created.space_id())?
        .ok_or("candidate chain missing")?;
    assert_eq!(chain.latest_generation(), 58);
    assert_eq!(chain.manifests().len(), 58);
    assert_eq!(
        chain
            .manifests()
            .last()
            .ok_or("latest manifest missing")?
            .revocations(),
        [SpaceRevocationV1::new(revoked.endpoint_id())]
    );
    candidate.shutdown().await?;
    let restarted_candidate = Runtime::start(candidate_config).await?;
    let restarted_status = restarted_candidate.handle().status().await?;
    assert_eq!(restarted_status.membership_count(), 1);
    assert!(restarted_status.normal_protocols_eligible());
    restarted_candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[test]
fn missing_intermediate_and_genesis_latest_only_never_validate() -> TestResult {
    // Given
    let state = TempState::new("chain-rejection")?;
    let config = StoreConfig::new(&state.0);
    let owner = member(endpoint(0x41)?, "owner")?;
    let mut repository = Repository::open(&config)?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    for generation in 1..=58 {
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            created.space_id(),
            10 + generation,
            SpaceManifestMembership::new(vec![owner.clone()], vec![]),
        ))?;
    }
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("chain missing")?;
    let pages = EnrollmentPage::paginate(&chain)?;

    // When
    let latest_page = pages.last().ok_or("latest page missing")?.encode()?;
    let mut missing = pages;
    missing.remove(2);
    let latest_only = vec![EnrollmentPage::decode(&latest_page)?];

    // Then
    assert!(validate_enrollment_pages(&missing).is_err());
    assert!(validate_enrollment_pages(&latest_only).is_err());
    Ok(())
}

fn member(endpoint_id: ma2a_core::EndpointId, label: &str) -> TestResultValue<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint_id,
        label.to_owned(),
        MemberCapabilities::new(true, false),
    )?)
}

fn endpoint(seed: u8) -> TestResultValue<ma2a_core::EndpointId> {
    let secret = ma2a_net::EndpointSecret::parse(&[seed; 32])?;
    Ok(secret.endpoint_id())
}
