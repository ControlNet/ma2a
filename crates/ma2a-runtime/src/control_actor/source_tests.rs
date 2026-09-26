use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::{
    InviteEntropy, MemberCapabilities, RequestId, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_net::{PrivateRelayProviderConfig, PrivateRelayProviderLocation, PrivateRelayTransport};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

use super::control_period;
use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock,
    control_sync::{ControlRoundScope, ControlRoundTrigger},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, crate::RuntimeError> {
        Ok(100)
    }
}

#[derive(Debug, Default)]
struct FailDuringPublicationClock {
    calls: AtomicU64,
}

impl RuntimeClock for FailDuringPublicationClock {
    fn now_ms(&self) -> Result<i64, crate::RuntimeError> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 1 {
            return Err(crate::error::RuntimeError::new(
                crate::error::RuntimeErrorKind::Clock,
            ));
        }
        Ok(100)
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-source-{label}-{}-{serial}",
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

struct OwnedRuntime {
    runtime: Runtime,
    _state: TempState,
    config: StoreConfig,
    endpoint_id: ma2a_core::EndpointId,
    space_id: ma2a_core::SpaceId,
    membership: SpaceManifestMembership,
}

#[tokio::test]
async fn startup_publishes_local_address_for_existing_space() -> TestResult {
    // Given
    let fixture = runtime_with_owned_space("startup-address").await?;

    // When
    let published_address =
        Repository::open(&fixture.config)?.address_record(fixture.space_id, fixture.endpoint_id)?;
    fixture.runtime.shutdown().await?;

    // Then
    assert!(published_address.is_some());
    Ok(())
}

#[tokio::test]
async fn failed_startup_publication_never_persists_ready_state() -> TestResult {
    // Given
    let fixture = runtime_with_owned_space("failed-startup-publication").await?;
    let config = fixture.config.clone();
    fixture.runtime.shutdown().await?;

    // When
    let failed = Runtime::start_with_clock(
        config.clone(),
        Arc::new(FailDuringPublicationClock::default()),
    )
    .await;

    // Then
    assert!(failed.is_err());
    let observation = Repository::open(&config)?
        .runtime_metadata()?
        .endpoint_observation();
    assert!(observation.is_some_and(|value| !value.ready));
    Runtime::start_with_clock(config, Arc::new(FixedClock))
        .await?
        .shutdown()
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_creation_publishes_local_address() -> TestResult {
    // Given
    let state = TempState::new("created-space-address")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let handle = runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();

    // When
    let space_id = handle.create_owned_space("local".to_owned()).await?;
    runtime.shutdown().await?;

    // Then
    let repository = Repository::open(&config)?;
    assert!(repository.address_record(space_id, endpoint_id)?.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_local_mutations_and_explicit_command_schedule_control_rounds() -> TestResult {
    // Given
    let fixture = runtime_with_owned_space("local-mutations").await?;
    let handle = fixture.runtime.handle();

    // When
    handle
        .advance_owned_space(OwnedSpaceUpdate::new(
            fixture.space_id,
            2,
            fixture.membership,
        ))
        .await?;
    handle.publish_address().await?;
    let relay_config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            "127.0.0.1:0".parse()?,
            "https://relay.example.invalid".parse()?,
        ),
        vec![fixture.space_id],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    handle
        .publish_relay_advertisements(relay_config, 600_100)
        .await?;
    handle.sync_control().await?;
    let schedules = handle.control_schedules();
    fixture.runtime.shutdown().await?;

    // Then
    assert!(schedules.contains(&ControlRoundTrigger::Startup));
    assert!(schedules.contains(&ControlRoundTrigger::ManifestAdvanced));
    assert!(schedules.contains(&ControlRoundTrigger::AddressAdvanced));
    assert!(schedules.contains(&ControlRoundTrigger::RelayAdvanced));
    assert!(
        schedules
            .iter()
            .any(|trigger| matches!(trigger, ControlRoundTrigger::Explicit(_)))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_enrollment_schedules_control_on_both_runtime_actors() -> TestResult {
    // Given
    let owner = runtime_with_owned_space("enrollment-owner").await?;
    let candidate_state = TempState::new("enrollment-candidate")?;
    let candidate_config = StoreConfig::new(&candidate_state.0);
    let candidate =
        Runtime::start_with_clock(candidate_config.clone(), Arc::new(FixedClock)).await?;
    let owner_handle = owner.runtime.handle();
    let candidate_handle = candidate.handle();
    let owner_endpoint_id = owner_handle.status().await?.endpoint_id();
    let candidate_endpoint_id = candidate_handle.status().await?.endpoint_id();
    let ticket = owner_handle
        .create_enrollment_invite(EnrollmentCreation::new(
            owner.space_id,
            300_000,
            InviteEntropy::from_bytes([0x31; 16], [0x41; 32]),
        )?)
        .await?;

    // When
    candidate_handle
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x51; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await?;
    let owner_schedules = owner_handle.control_schedules();
    let candidate_schedules = candidate_handle.control_schedules();
    candidate.shutdown().await?;
    owner.runtime.shutdown().await?;

    // Then
    assert!(owner_schedules.contains(&ControlRoundTrigger::EnrollmentCompleted));
    assert!(
        candidate_schedules.contains(&ControlRoundTrigger::Explicit(ControlRoundScope::peer(
            owner_endpoint_id
        )))
    );
    let repository = Repository::open(&candidate_config)?;
    assert!(
        repository
            .address_record(owner.space_id, candidate_endpoint_id)?
            .is_some()
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn periodic_timer_schedules_a_control_round() -> TestResult {
    // Given
    let fixture = runtime_with_owned_space("periodic").await?;
    let handle = fixture.runtime.handle();
    let endpoint_id = handle.status().await?.endpoint_id();
    // When
    tokio::time::advance(control_period(endpoint_id)).await;
    // A status command is prioritized ahead of a ready maintenance tick. Wait
    // for the observable scheduling event, not for an unrelated mailbox reply.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !handle
            .control_schedules()
            .contains(&ControlRoundTrigger::Periodic)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let schedules = handle.control_schedules();
    fixture.runtime.shutdown().await?;

    // Then
    assert!(schedules.contains(&ControlRoundTrigger::Periodic));
    Ok(())
}

async fn runtime_with_owned_space(label: &str) -> TestResultValue<OwnedRuntime> {
    let state = TempState::new(label)?;
    let config = StoreConfig::new(&state.0);
    let bootstrap = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let endpoint_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&config)?;
    let member = SpaceMemberV1::new(
        endpoint_id,
        "local".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1,
        member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let space_id = created.space_id();
    let membership = SpaceManifestMembership::new(created.chain().members().to_vec(), Vec::new());
    drop(repository);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    Ok(OwnedRuntime {
        runtime,
        _state: state,
        config,
        endpoint_id,
        space_id,
        membership,
    })
}
