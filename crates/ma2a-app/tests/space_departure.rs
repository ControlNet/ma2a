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
    EndpointId, EnrollmentPage, InviteEntropy, MAX_ENROLLMENT_ARTIFACTS_PER_PAGE,
    MemberCapabilities, RequestId, SignedInviteTicket, SpaceId, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1, default_member_label,
};
use ma2a_runtime::{
    EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock, SpaceDepartureErrorCode,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

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
///
/// The creator's member label names the Endpoint, never the Space, so this
/// helper must not pass `name` where a member label belongs.
fn create_named_space(config: &StoreConfig, owner: EndpointId, name: &str) -> TestValue<SpaceId> {
    let member = SpaceMemberV1::new(
        owner,
        default_member_label(owner),
        MemberCapabilities::new(true, true),
    )?;
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

/// Two live Runtimes sharing one named Space, joined the way the CLI joins.
struct JoinedSpace {
    owner: Runtime,
    member: Runtime,
    owner_config: StoreConfig,
    member_config: StoreConfig,
    owner_id: EndpointId,
    member_id: EndpointId,
    space_id: SpaceId,
    /// The invitation the member already consumed.
    consumed: SignedInviteTicket,
    _states: (TempState, TempState),
}

impl JoinedSpace {
    async fn new(label: &str, name: &str) -> TestValue<Self> {
        let owner_state = TempState::new(&format!("{label}-owner"))?;
        let member_state = TempState::new(&format!("{label}-member"))?;
        let owner_config = StoreConfig::new(&owner_state.0);
        let member_config = StoreConfig::new(&member_state.0);
        let bootstrap = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
        let owner_id = bootstrap.handle().status().await?.endpoint_id();
        bootstrap.shutdown().await?;
        let space_id = create_named_space(&owner_config, owner_id, name)?;
        let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
        let member = Runtime::start_with_clock(member_config.clone(), clock()).await?;
        let member_id = member.handle().status().await?.endpoint_id();
        let consumed = enroll((&owner, &member), space_id, (0x11, 0x33)).await?;
        Ok(Self {
            owner,
            member,
            owner_config,
            member_config,
            owner_id,
            member_id,
            space_id,
            consumed,
            _states: (owner_state, member_state),
        })
    }

    async fn shutdown(self) -> TestResult {
        self.member.shutdown().await?;
        self.owner.shutdown().await?;
        Ok(())
    }
}

/// Invites and redeems exactly as the CLI does, labelling the joining member
/// from its own Endpoint identity.
async fn enroll(
    pair: (&Runtime, &Runtime),
    space_id: SpaceId,
    seeds: (u8, u8),
) -> TestValue<SignedInviteTicket> {
    let (owner, member) = pair;
    let member_id = member.handle().status().await?.endpoint_id();
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::from_bytes([seeds.0; 16], [seeds.0 ^ 0xff; 32]),
        )?)
        .await?;
    member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket.clone(),
            RequestId::try_from([seeds.1; 16].as_slice())?,
            default_member_label(member_id),
        ))
        .await?;
    Ok(ticket)
}

/// Reads the signed member labels straight from the persisted chain.
fn member_labels(config: &StoreConfig, space_id: SpaceId) -> TestValue<Vec<String>> {
    Ok(Repository::open(config)?
        .load_space_chain(space_id)?
        .ok_or("chain missing")?
        .members()
        .iter()
        .map(|member| member.label().to_owned())
        .collect())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_joining_member_learns_the_shared_name_without_being_labelled_by_it() -> TestResult {
    // Given
    let space = JoinedSpace::new("named", "lab").await?;

    // When
    let joined = space.member.handle().snapshot().await?;

    // Then: the joining Endpoint knows the shared name without local configuration.
    assert_eq!(shared_name(&joined, space.space_id)?, "lab");

    // A Space name and a member label are separate signed facts. Neither the
    // creator nor the joiner may be labelled with the Space's name, and no
    // placeholder may stand in for an Endpoint identity.
    let labels = member_labels(&space.owner_config, space.space_id)?;
    assert_eq!(labels.len(), 2, "{labels:?}");
    for label in &labels {
        assert_ne!(label, "lab", "a member label must not be the Space name");
        assert_ne!(
            label, "local-endpoint",
            "placeholder member label persisted"
        );
    }
    assert!(
        labels.contains(&default_member_label(space.owner_id)),
        "{labels:?}"
    );
    assert!(
        labels.contains(&default_member_label(space.member_id)),
        "{labels:?}"
    );
    assert_eq!(
        member_labels(&space.member_config, space.space_id)?,
        labels,
        "both sides must read the same signed member labels"
    );
    space.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_leaves_through_an_authority_signed_removal() -> TestResult {
    // Given
    let space = JoinedSpace::new("leave", "lab").await?;

    // The owner cannot leave its own Space.
    let refused = space
        .owner
        .handle()
        .leave_space(space.space_id, RequestId::try_from([0x44; 16].as_slice())?)
        .await
        .err()
        .ok_or("the owner must not be able to leave")?;
    assert_eq!(
        refused.code(),
        SpaceDepartureErrorCode::OWNER_CANNOT_LEAVE,
        "{refused}"
    );
    assert_eq!(space.owner.handle().status().await?.membership_count(), 1);

    // Departure resolves the authority through the Space's own signed address
    // record and the private lookup it feeds; the member was never handed a raw
    // transport address for the owner and no public discovery is configured.
    assert!(
        Repository::open(&space.member_config)?
            .address_record(space.space_id, space.owner_id)?
            .is_some(),
        "the member must resolve the owner from its signed Space address record"
    );

    // When
    space
        .member
        .handle()
        .leave_space(space.space_id, RequestId::try_from([0x55; 16].as_slice())?)
        .await?;

    // Then
    assert_eq!(space.member.handle().status().await?.membership_count(), 0);
    let owner_chain = Repository::open(&space.owner_config)?
        .load_space_chain(space.space_id)?
        .ok_or("owner chain missing")?;
    assert!(
        !owner_chain
            .members()
            .iter()
            .any(|entry| entry.endpoint_id() == space.member_id),
        "the owner's signed membership must no longer include the departed member"
    );
    assert!(
        owner_chain
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == space.member_id),
        "departure must be recorded as a cryptographic revocation"
    );
    let member_chain = Repository::open(&space.member_config)?
        .load_space_chain(space.space_id)?
        .ok_or("member chain missing")?;
    assert_eq!(
        member_chain.latest_generation(),
        owner_chain.latest_generation(),
        "the leaving member must persist the authority-signed removal"
    );
    space.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_departed_member_cannot_replay_its_invite_but_may_rejoin() -> TestResult {
    // Given
    let space = JoinedSpace::new("rejoin", "lab").await?;
    space
        .member
        .handle()
        .leave_space(space.space_id, RequestId::try_from([0x55; 16].as_slice())?)
        .await?;
    assert_eq!(space.member.handle().status().await?.membership_count(), 0);

    // When: a consumed invitation is replayed with a fresh identifier, and again
    // with the original one the owner answers from its recorded response.
    let replayed = space
        .member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            space.consumed.clone(),
            RequestId::try_from([0x99; 16].as_slice())?,
            default_member_label(space.member_id),
        ))
        .await;
    let retried = space
        .member
        .handle()
        .clone()
        .redeem_enrollment(EnrollmentAttempt::new(
            space.consumed.clone(),
            RequestId::try_from([0x33; 16].as_slice())?,
            default_member_label(space.member_id),
        ))
        .await;

    // Then
    assert!(
        replayed.is_err(),
        "a consumed invitation must not be reusable"
    );
    assert!(
        retried.is_err(),
        "a recorded retry must not restore membership"
    );
    assert!(
        Repository::open(&space.member_config)?
            .memberships_for(space.member_id)?
            .is_empty(),
        "replay must not restore durable membership"
    );
    assert_eq!(
        space.member.handle().status().await?.membership_count(),
        0,
        "replay must not restore reported membership"
    );

    // A fresh invitation still works, and the historical revocation stays behind.
    enroll((&space.owner, &space.member), space.space_id, (0x66, 0x88)).await?;
    assert_eq!(space.member.handle().status().await?.membership_count(), 1);
    let rejoined = space.member.handle().snapshot().await?;
    assert_eq!(shared_name(&rejoined, space.space_id)?, "lab");
    let chain = Repository::open(&space.member_config)?
        .load_space_chain(space.space_id)?
        .ok_or("chain missing")?;
    assert!(
        chain
            .members()
            .iter()
            .any(|entry| entry.endpoint_id() == space.member_id),
        "the rejoining member must be a current member again"
    );
    assert!(
        !chain
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == space.member_id),
        "re-adding must clear the current revocation rather than rewrite history"
    );
    space.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_an_unreachable_authority_fails_without_removing_local_membership() -> TestResult {
    // Given
    let space = JoinedSpace::new("offline", "offline").await?;
    let member_id = space.member_id;
    let space_id = space.space_id;
    let member_config = space.member_config.clone();
    space.owner.shutdown().await?;

    // When
    let failure = space
        .member
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
    assert_eq!(space.member.handle().status().await?.membership_count(), 1);
    assert_eq!(
        Repository::open(&member_config)?
            .memberships_for(member_id)?
            .len(),
        1,
        "a failed departure must leave signed membership untouched"
    );
    space.member.shutdown().await?;
    Ok(())
}

/// A long-lived Space's history does not fit one transport frame, so departure
/// streams it through the same bounded pagination enrollment uses.
///
/// The history is grown directly in the owner's Store before either Runtime
/// starts, so the fixture costs signatures rather than live control rounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_space_history_departs_through_bounded_pagination() -> TestResult {
    // Given
    let owner_state = TempState::new("paged-owner")?;
    let member_state = TempState::new("paged-member")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let member_config = StoreConfig::new(&member_state.0);
    let bootstrap = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let space_id = create_named_space(&owner_config, owner_id, "paged")?;
    let owner_member = SpaceMemberV1::new(
        owner_id,
        default_member_label(owner_id),
        MemberCapabilities::new(true, true),
    )?;
    // One manifest past a full page is all this needs: the point is that the
    // departure chain spans more than one page, not that the history is long.
    // Every extra generation is a signed advance that costs real time here.
    let generations = u64::try_from(MAX_ENROLLMENT_ARTIFACTS_PER_PAGE)?;
    let mut repository = Repository::open(&owner_config)?;
    for generation in 0..generations {
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            u64::try_from(NOW_MS)? + generation + 1,
            SpaceManifestMembership::new(vec![owner_member.clone()], Vec::new()),
        ))?;
    }
    drop(repository);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let member = Runtime::start_with_clock(member_config.clone(), clock()).await?;
    let member_id = member.handle().status().await?.endpoint_id();
    let _ticket = enroll((&owner, &member), space_id, (0x51, 0x53)).await?;
    let joined = Repository::open(&owner_config)?
        .load_space_chain(space_id)?
        .ok_or("joined chain missing")?;
    assert!(
        EnrollmentPage::paginate(&joined)?.len() > 1,
        "the fixture must need more than one departure page"
    );

    // When
    member
        .handle()
        .leave_space(space_id, RequestId::try_from([0x54; 16].as_slice())?)
        .await?;

    // Then
    assert_eq!(member.handle().status().await?.membership_count(), 0);
    let departed = Repository::open(&member_config)?
        .load_space_chain(space_id)?
        .ok_or("member chain missing")?;
    assert_eq!(
        departed.latest_generation(),
        joined.latest_generation() + 1,
        "the whole paginated history must be persisted, not just the last page"
    );
    assert_eq!(departed.manifests().len(), joined.manifests().len() + 1);
    assert!(
        !departed
            .members()
            .iter()
            .any(|entry| entry.endpoint_id() == member_id)
    );
    assert!(
        departed
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == member_id)
    );
    member.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}
