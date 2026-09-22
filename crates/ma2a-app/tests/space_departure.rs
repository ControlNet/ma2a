//! End-to-end coverage for shared Space names and member-initiated departure.

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
    EndpointId, InviteEntropy, MemberCapabilities, RequestId, SpaceId, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_runtime::{
    EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock, SpaceDepartureErrorCode,
};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);
const NOW_MS: i64 = 1_700_000_000_000;

#[derive(Debug)]
struct TestClock(AtomicI64);

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> Result<i64, ma2a_runtime::RuntimeError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-departure-{label}-{}-{serial}",
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

fn clock() -> Arc<TestClock> {
    Arc::new(TestClock(AtomicI64::new(NOW_MS)))
}

/// Creates a named owned Space directly in the owner's Store, mirroring what
/// `ma2a space create <NAME>` commits through the Runtime.
fn create_named_space(config: &StoreConfig, owner: EndpointId, name: &str) -> TestValue<SpaceId> {
    let member = SpaceMemberV1::new(owner, name.to_owned(), MemberCapabilities::new(true, true))?;
    let mut repository = Repository::open(config)?;
    Ok(repository
        .create_owned_space(
            &SpaceCreation::new(
                u64::try_from(NOW_MS)?,
                member,
                SpacePolicyV1::phase_one_default(),
            )
            .with_name(name.to_owned()),
        )?
        .space_id())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_learns_the_shared_name_then_leaves_and_rejoins_the_same_space() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let member_state = TempState::new("member")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let member_config = StoreConfig::new(&member_state.0);
    let bootstrap = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let space_id = create_named_space(&owner_config, owner_id, "lab")?;
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let member = Runtime::start_with_clock(member_config.clone(), clock()).await?;
    let member_id = member.handle().status().await?.endpoint_id();
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::from_bytes([0x11; 16], [0x22; 32]),
        )?)
        .await?;

    // When
    member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x33; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;

    // Then: the joining Endpoint knows the shared Space name without local configuration.
    let joined = member.handle().snapshot().await?;
    assert_eq!(shared_name(&joined, space_id)?, "lab");

    // The owner cannot leave its own Space.
    let refused = owner
        .handle()
        .leave_space(space_id, RequestId::try_from([0x44; 16].as_slice())?)
        .await
        .err()
        .ok_or("the owner must not be able to leave")?;
    assert_eq!(
        refused.code(),
        SpaceDepartureErrorCode::OWNER_CANNOT_LEAVE,
        "{refused}"
    );
    assert_eq!(owner.handle().status().await?.membership_count(), 1);

    // A member's departure produces a real authority-signed removal on both sides.
    member
        .handle()
        .leave_space(space_id, RequestId::try_from([0x55; 16].as_slice())?)
        .await?;
    assert_eq!(member.handle().status().await?.membership_count(), 0);
    let owner_chain = Repository::open(&owner_config)?
        .load_space_chain(space_id)?
        .ok_or("owner chain missing")?;
    assert!(
        !owner_chain
            .members()
            .iter()
            .any(|entry| entry.endpoint_id() == member_id),
        "the owner's signed membership must no longer include the departed member"
    );
    assert!(
        owner_chain
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == member_id),
        "departure must be recorded as a cryptographic revocation"
    );
    let member_chain = Repository::open(&member_config)?
        .load_space_chain(space_id)?
        .ok_or("member chain missing")?;
    assert_eq!(
        member_chain.latest_generation(),
        owner_chain.latest_generation(),
        "the leaving member must persist the authority-signed removal"
    );
    assert!(
        Repository::open(&member_config)?
            .memberships_for(member_id)?
            .is_empty()
    );

    // A departed Endpoint may rejoin through a fresh invite.
    let rejoin = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::from_bytes([0x66; 16], [0x77; 32]),
        )?)
        .await?;
    member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            rejoin,
            RequestId::try_from([0x88; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    assert_eq!(member.handle().status().await?.membership_count(), 1);
    let rejoined = member.handle().snapshot().await?;
    assert_eq!(shared_name(&rejoined, space_id)?, "lab");
    member.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_an_unreachable_authority_fails_without_removing_local_membership() -> TestResult {
    // Given
    let owner_state = TempState::new("offline-owner")?;
    let member_state = TempState::new("offline-member")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let member_config = StoreConfig::new(&member_state.0);
    let bootstrap = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let space_id = create_named_space(&owner_config, owner_id, "offline")?;
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let member = Runtime::start_with_clock(member_config.clone(), clock()).await?;
    let member_id = member.handle().status().await?.endpoint_id();
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::from_bytes([0x31; 16], [0x32; 32]),
        )?)
        .await?;
    member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x34; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;

    // When
    let failure = member
        .handle()
        .leave_space(space_id, RequestId::try_from([0x35; 16].as_slice())?)
        .await
        .err()
        .ok_or("leaving an unreachable authority must fail")?;

    // Then
    assert_eq!(
        failure.code(),
        SpaceDepartureErrorCode::UNREACHABLE,
        "{failure}"
    );
    assert_eq!(member.handle().status().await?.membership_count(), 1);
    assert_eq!(
        Repository::open(&member_config)?
            .memberships_for(member_id)?
            .len(),
        1,
        "a failed departure must leave signed membership untouched"
    );
    member.shutdown().await?;
    Ok(())
}

fn shared_name(
    snapshot: &ma2a_runtime::api::RuntimeSnapshot,
    space_id: SpaceId,
) -> TestValue<String> {
    snapshot
        .spaces()
        .iter()
        .find(|space| space.space_id() == space_id)
        .map(|space| space.name().to_owned())
        .ok_or_else(|| "the Space is absent from this snapshot".into())
}
