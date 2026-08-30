//! Runtime relay reachability startup and persistence integration.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{
    MemberCapabilities, RelayReachability, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_runtime::Runtime;
use ma2a_store::{OwnedSpaceUpdate, RelayObservation, Repository, SpaceCreation, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(name: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-reachability-{name}-{}-{serial}",
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

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_spaces_without_common_home_or_public_fallback_are_degraded() -> TestResult {
    // Given
    let state = TempState::new("no-common-home")?;
    let config = StoreConfig::new(state.path());
    let bootstrap = Runtime::start(config.clone()).await?;
    let local_endpoint_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&config)?;
    create_local_space(&mut repository, local_endpoint_id, 10)?;
    create_local_space(&mut repository, local_endpoint_id, 20)?;
    drop(repository);

    // When
    let runtime = Runtime::start(config).await?;
    let status = runtime.handle().status().await?;
    runtime.shutdown().await?;

    // Then
    assert_eq!(
        status.relay_reachability(),
        RelayReachability::DegradedNoCommonHome
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_space_runtime_does_not_report_degraded_common_home() -> TestResult {
    // Given
    let state = TempState::new("zero-space")?;

    // When
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let status = runtime.handle().status().await?;
    runtime.shutdown().await?;

    // Then
    assert_eq!(
        status.relay_reachability(),
        RelayReachability::NoActiveSpaces
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_discards_stale_effective_relay_observations() -> TestResult {
    // Given
    let state = TempState::new("stale-effective-relay")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.replace_relay_observations(&[RelayObservation {
        relay_url: "https://stale.example.invalid".to_owned(),
        observed_at_ms: 1,
        expires_at_ms: 2,
        reachable: true,
        latency_ms: None,
        observed_state: vec![1],
    }])?;
    drop(repository);

    // When
    let runtime = Runtime::start(config.clone()).await?;
    runtime.shutdown().await?;

    // Then
    assert!(Repository::open(&config)?.relay_observations()?.is_empty());
    Ok(())
}

fn create_local_space(
    repository: &mut Repository,
    local_endpoint_id: ma2a_core::EndpointId,
    issued_at_ms: u64,
) -> TestResult {
    let local_member = SpaceMemberV1::new(
        local_endpoint_id,
        format!("local-{issued_at_ms}"),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        issued_at_ms,
        local_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        issued_at_ms + 1,
        SpaceManifestMembership::new(vec![local_member], Vec::new()),
    ))?;
    Ok(())
}
