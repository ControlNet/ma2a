use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::Duration,
};

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::{Repository, StoreConfig};
use tokio::time::timeout;

use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock,
    actor::maintenance::FaultPoint,
    error::{RuntimeError, RuntimeErrorKind},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const NOW_MS: i64 = 1_700_000_000_000;
const ADDRESS_REFRESH_MS: i64 = 300_000;
const PROMPT: Duration = Duration::from_secs(10);
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct SteppingClock {
    now_ms: AtomicI64,
    step_ms: AtomicI64,
}

impl SteppingClock {
    const fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
            step_ms: AtomicI64::new(0),
        }
    }

    fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    fn set_step(&self, step_ms: i64) {
        self.step_ms.store(step_ms, Ordering::SeqCst);
    }
}

impl RuntimeClock for SteppingClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        let step = self.step_ms.load(Ordering::SeqCst);
        Ok(self.now_ms.fetch_add(step, Ordering::SeqCst))
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-enroll-step-{label}-{}-{serial}",
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

async fn start_runtime(
    label: &str,
    now_ms: i64,
) -> Result<(TempState, StoreConfig, Arc<SteppingClock>, Runtime), Box<dyn Error + Send + Sync>> {
    let dir = TempState::new(label)?;
    let config = StoreConfig::new(&dir.0);
    let clock = Arc::new(SteppingClock::new(now_ms));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<SteppingClock>::clone(&clock)).await?;
    Ok((dir, config, clock, runtime))
}

async fn invite(
    owner: &Runtime,
    space: ma2a_core::SpaceId,
) -> Result<ma2a_core::SignedInviteTicket, Box<dyn Error + Send + Sync>> {
    Ok(owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?)
}

async fn redeem_as(
    candidate: &Runtime,
    ticket: ma2a_core::SignedInviteTicket,
    request_byte: u8,
) -> Result<crate::EstablishedEnrollment, Box<dyn Error + Send + Sync>> {
    Ok(timeout(
        PROMPT,
        candidate.handle().redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([request_byte; 16].as_slice())?,
            "candidate".to_owned(),
        )),
    )
    .await??)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_commit_address_publication_succeeds_under_stepping_clock() -> TestResult {
    let (_owner_dir, owner_config, owner_clock, owner) =
        start_runtime("pre-commit-owner", NOW_MS).await?;
    let (_cand_dir, _cand_config, _cand_clock, candidate) =
        start_runtime("pre-commit-cand", NOW_MS + 10_000).await?;

    for _ in 0..2 {
        owner.handle().fail_background_once(
            FaultPoint::AddressObservation,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
    let space = owner
        .handle()
        .create_owned_space("step-missing".to_owned())
        .await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    assert!(
        Repository::open(&owner_config)?
            .address_record(space, owner_id)?
            .is_none()
    );

    let ticket = invite(&owner, space).await?;
    owner.handle().clear_background_faults();
    owner_clock.set_step(10);

    let joined = redeem_as(&candidate, ticket, 0xb1).await?;
    let candidate_id = candidate.handle().status().await?.endpoint_id();
    assert!(
        joined
            .chain()
            .members()
            .iter()
            .any(|m| m.endpoint_id() == candidate_id)
    );
    assert!(
        Repository::open(&owner_config)?
            .address_record(space, owner_id)?
            .is_some()
    );

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_commit_address_renewal_succeeds_under_stepping_clock() -> TestResult {
    let (_owner_dir, owner_config, owner_clock, owner) =
        start_runtime("post-commit-owner", NOW_MS).await?;
    let (_cand_dir, _cand_config, _cand_clock, candidate) =
        start_runtime("post-commit-cand", NOW_MS + ADDRESS_REFRESH_MS + 10_000).await?;

    let space = owner
        .handle()
        .create_owned_space("step-renew".to_owned())
        .await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let initial_seq = Repository::open(&owner_config)?
        .address_record(space, owner_id)?
        .ok_or("initial owner address missing")?
        .sequence();

    owner_clock.set(NOW_MS + ADDRESS_REFRESH_MS);
    let ticket = invite(&owner, space).await?;
    owner_clock.set_step(10);

    let joined = redeem_as(&candidate, ticket, 0xb2).await?;
    let candidate_id = candidate.handle().status().await?.endpoint_id();
    assert!(
        joined
            .chain()
            .members()
            .iter()
            .any(|m| m.endpoint_id() == candidate_id)
    );
    let refreshed_seq = Repository::open(&owner_config)?
        .address_record(space, owner_id)?
        .ok_or("refreshed owner address missing")?
        .sequence();
    assert!(refreshed_seq > initial_seq);

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_address_renewal_succeeds_under_stepping_clock() -> TestResult {
    let (_owner_dir, owner_config, owner_clock, owner) =
        start_runtime("retry-renew-owner", NOW_MS).await?;
    let space = owner
        .handle()
        .create_owned_space("step-retry".to_owned())
        .await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let initial_seq = Repository::open(&owner_config)?
        .address_record(space, owner_id)?
        .ok_or("initial owner address missing")?
        .sequence();

    owner_clock.set(NOW_MS + ADDRESS_REFRESH_MS - 50_000);
    let ticket = invite(&owner, space).await?;
    let attempt = EnrollmentAttempt::new(
        ticket,
        RequestId::try_from([0xb3; 16].as_slice())?,
        "retry-candidate".to_owned(),
    );
    let raw_request = crate::enrollment::encode_attempt(&attempt)?;
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let endpoint =
        ma2a_net::RuntimeEndpoint::bind(ma2a_net::EndpointSecret::generate(), tx, None).await?;

    let (first_status, _) = timeout(
        PROMPT,
        endpoint.exchange_redemption(attempt.owner_addr(), raw_request.clone()),
    )
    .await??;
    assert_eq!(first_status, 0);

    owner_clock.set(NOW_MS + ADDRESS_REFRESH_MS);
    owner_clock.set_step(10);
    let (retry_status, retry_pages) = timeout(
        PROMPT,
        endpoint.exchange_redemption(attempt.owner_addr(), raw_request),
    )
    .await??;

    assert_eq!(retry_status, 0);
    let _bootstrap = ma2a_core::EnrollmentBootstrap::decode_frames(&retry_pages)?;
    let refreshed_seq = Repository::open(&owner_config)?
        .address_record(space, owner_id)?
        .ok_or("refreshed owner address missing")?
        .sequence();
    assert!(refreshed_seq > initial_seq);

    endpoint.shutdown().await?;
    timeout(PROMPT, owner.shutdown()).await??;
    Ok(())
}
