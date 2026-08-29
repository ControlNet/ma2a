//! End-to-end enrollment denial coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use iroh_tickets::Ticket as _;
use ma2a_core::{
    InviteEntropy, MemberCapabilities, RequestId, SignedInviteTicket, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_runtime::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentErrorCode, Runtime, RuntimeClock,
};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct TestClock(AtomicI64);

impl TestClock {
    fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

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
            "ma2a-enrollment-denial-{label}-{}-{serial}",
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
async fn expiry_cancellation_and_cross_space_leave_state_unchanged() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
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
    let clock = Arc::new(TestClock(AtomicI64::new(201)));
    let clock_for_runtime = Arc::clone(&clock);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock_for_runtime).await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_state.0)).await?;
    let candidate_before = candidate.handle().status().await?;
    let generation = chain_generation(&owner_config, first.space_id())?;
    let context = DenialContext {
        owner: &owner,
        candidate: &candidate,
        owner_config: &owner_config,
        first_space: first.space_id(),
        second_space: second.space_id(),
        clock: &clock,
        generation,
    };

    // When
    assert_expired(&context).await?;
    assert_cancelled(&context).await?;
    assert_cross_space(&context).await?;

    // Then
    let candidate_after = candidate.handle().status().await?;
    assert_eq!(candidate_after.revision(), candidate_before.revision());
    assert_eq!(candidate_after.membership_count(), 0);
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

struct DenialContext<'a> {
    owner: &'a Runtime,
    candidate: &'a Runtime,
    owner_config: &'a StoreConfig,
    first_space: ma2a_core::SpaceId,
    second_space: ma2a_core::SpaceId,
    clock: &'a TestClock,
    generation: u64,
}

async fn assert_expired(context: &DenialContext<'_>) -> TestResult {
    let secret = [0x62; 32];
    let ticket = context
        .owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            context.first_space,
            100,
            InviteEntropy::from_bytes([0x61; 16], secret),
        )?)
        .await?;
    context.clock.set(302);
    assert!(
        !fs::read(context.owner_config.database_path())?
            .windows(32)
            .any(|window| window == secret)
    );
    let before = context.owner.handle().status().await?;
    let result = context
        .candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x63; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await;
    let Err(error) = result else {
        return Err("expired invite succeeded".into());
    };
    assert_eq!(error.code(), EnrollmentErrorCode::EXPIRED);
    assert_unchanged(context, before.revision()).await
}

async fn assert_cancelled(context: &DenialContext<'_>) -> TestResult {
    context.clock.set(302);
    let ticket = context
        .owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            context.first_space,
            100,
            InviteEntropy::from_bytes([0x64; 16], [0x65; 32]),
        )?)
        .await?;
    context
        .owner
        .handle()
        .cancel_enrollment_invite(ticket.invitation_id())
        .await?;
    let before = context.owner.handle().status().await?;
    let result = context
        .candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x66; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await;
    let Err(error) = result else {
        return Err("cancelled invite succeeded".into());
    };
    assert_eq!(error.code(), EnrollmentErrorCode::CANCELLED);
    assert_unchanged(context, before.revision()).await
}

async fn assert_cross_space(context: &DenialContext<'_>) -> TestResult {
    context.clock.set(501);
    let ticket = context
        .owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            context.second_space,
            100,
            InviteEntropy::from_bytes([0x67; 16], [0x68; 32]),
        )?)
        .await?;
    let mut bytes = ticket.encode_bytes();
    let first = *context
        .first_space
        .as_bytes()
        .first()
        .ok_or("first Space id is empty")?;
    let second = *context
        .second_space
        .as_bytes()
        .first()
        .ok_or("second Space id is empty")?;
    *bytes.get_mut(17).ok_or("ticket Space byte missing")? ^= first ^ second;
    let ticket = SignedInviteTicket::decode_bytes(&bytes)?;
    let before = context.owner.handle().status().await?;
    let result = context
        .candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x69; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await;
    let Err(error) = result else {
        return Err("cross-Space ticket succeeded".into());
    };
    assert_eq!(error.code(), EnrollmentErrorCode::INVALID_TICKET);
    assert_unchanged(context, before.revision()).await
}

async fn assert_unchanged(context: &DenialContext<'_>, revision: u64) -> TestResult {
    assert_eq!(context.owner.handle().status().await?.revision(), revision);
    assert_eq!(
        chain_generation(context.owner_config, context.first_space)?,
        context.generation
    );
    Ok(())
}

fn chain_generation(config: &StoreConfig, space_id: ma2a_core::SpaceId) -> TestResultValue<u64> {
    Ok(Repository::open(config)?
        .load_space_chain(space_id)?
        .ok_or("chain missing")?
        .latest_generation())
}

fn member(endpoint_id: ma2a_core::EndpointId, label: &str) -> TestResultValue<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint_id,
        label.to_owned(),
        MemberCapabilities::new(true, false),
    )?)
}
