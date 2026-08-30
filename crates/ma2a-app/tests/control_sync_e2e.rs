//! End-to-end active-dial control synchronization coverage.

use std::{
    error::Error,
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, InviteEntropy,
    MemberCapabilities, RequestId, SpaceAddressRecordV1, SpaceAuthorizationView,
    SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{AddressRecordTarget, AddressRecordValidator};
use ma2a_runtime::{EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock};
use ma2a_store::{
    KeyKind, KeyReference, KeyStore, OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig,
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
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
    fn new(label: &str) -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-{label}-{}-{serial}",
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
async fn explicit_round_converges_shared_spaces_without_leaking_private_space() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let candidate_config = StoreConfig::new(&candidate_state.0);
    let clock = || Arc::new(TestClock(AtomicI64::new(NOW_MS)));
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_status = owner.handle().status().await?;
    owner.shutdown().await?;
    let owner_member = SpaceMemberV1::new(
        owner_status.endpoint_id(),
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let mut owner_repository = Repository::open(&owner_config)?;
    let first = owner_repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS)?,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let second = owner_repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS + 1)?,
        owner_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let private = owner_repository.create_owned_space(&SpaceCreation::new(
        u64::try_from(NOW_MS + 2)?,
        owner_member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let shared_spaces = [first.space_id(), second.space_id()];
    drop(owner_repository);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;
    enroll_spaces(&owner, &candidate, shared_spaces).await?;
    candidate.shutdown().await?;
    owner.shutdown().await?;
    let mut owner_repository = Repository::open(&owner_config)?;
    for (offset, space_id) in shared_spaces.into_iter().enumerate() {
        let chain = owner_repository
            .load_space_chain(space_id)?
            .ok_or("owner chain missing")?;
        owner_repository.advance_owned_space(&OwnedSpaceUpdate::new(
            space_id,
            u64::try_from(NOW_MS + 10 + i64::try_from(offset)?)?,
            SpaceManifestMembership::new(chain.members().to_vec(), vec![]),
        ))?;
    }
    let owner_port = owner_repository
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let owner_secret = endpoint_secret(&owner_state.0)?;
    let owner_records = shared_spaces
        .into_iter()
        .map(|space_id| address_record(space_id, &owner_secret, owner_port))
        .collect::<TestResultValue<Vec<_>>>()?;
    for record in &owner_records {
        persist_address(&mut owner_repository, record, u64::try_from(NOW_MS)?)?;
    }
    drop(owner_repository);
    let mut candidate_repository = Repository::open(&candidate_config)?;
    for record in &owner_records {
        persist_address(&mut candidate_repository, record, u64::try_from(NOW_MS)?)?;
    }
    let candidate_port = candidate_repository
        .endpoint_bind_port()?
        .ok_or("candidate bind port missing")?;
    let candidate_secret = endpoint_secret(&candidate_state.0)?;
    let candidate_records = shared_spaces
        .into_iter()
        .map(|space_id| address_record(space_id, &candidate_secret, candidate_port))
        .collect::<TestResultValue<Vec<_>>>()?;
    for record in &candidate_records {
        persist_address(&mut candidate_repository, record, u64::try_from(NOW_MS)?)?;
    }
    drop(candidate_repository);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;

    // When
    tokio::time::timeout(Duration::from_secs(15), candidate.handle().sync_control()).await??;

    // Then
    let candidate_repository = Repository::open(&candidate_config)?;
    let owner_repository = Repository::open(&owner_config)?;
    for (space_id, candidate_record) in shared_spaces.into_iter().zip(&candidate_records) {
        let synchronized = candidate_repository
            .load_space_chain(space_id)?
            .ok_or("candidate chain missing")?;
        assert_eq!(synchronized.latest_generation(), 2);
        let imported = owner_repository
            .address_record(space_id, candidate_secret.public().into())?
            .ok_or("candidate address was not pushed")?;
        assert_eq!(imported.signed_record(), candidate_record.canonical_bytes());
    }
    assert!(
        candidate_repository
            .load_space_chain(private.space_id())?
            .is_none()
    );
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

async fn enroll_spaces(
    owner: &Runtime,
    candidate: &Runtime,
    spaces: [ma2a_core::SpaceId; 2],
) -> TestResult {
    for (offset, space_id) in spaces.into_iter().enumerate() {
        let seed = u8::try_from(offset)?;
        let ticket = owner
            .handle()
            .create_enrollment_invite(EnrollmentCreation::new(
                space_id,
                300_000,
                InviteEntropy::from_bytes([0x31 + seed; 16], [0x41 + seed; 32]),
            )?)
            .await?;
        candidate
            .handle()
            .redeem_enrollment(EnrollmentAttempt::new(
                ticket,
                RequestId::try_from([0x51 + seed; 16].as_slice())?,
                "candidate".to_owned(),
            ))
            .await?;
    }
    Ok(())
}

fn endpoint_secret(state_dir: &Path) -> TestResultValue<SecretKey> {
    let reference = KeyReference::parse("endpoint-identity-v1")?;
    let protected = KeyStore::open(state_dir)?.read(KeyKind::Endpoint, &reference)?;
    let bytes = <[u8; 32]>::try_from(protected.as_ref())?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn address_record(
    space_id: ma2a_core::SpaceId,
    secret: &SecretKey,
    port: u16,
) -> TestResultValue<ma2a_core::SignedSpaceAddressRecordV1> {
    Ok(SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, secret.public().into()),
        AddressRecordValidity::new(1, u64::try_from(NOW_MS)?, u64::try_from(NOW_MS + 600_000)?)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(SocketAddr::new(
            Ipv4Addr::LOCALHOST.into(),
            port,
        ))])?,
    )
    .sign(secret)?)
}

fn persist_address(
    repository: &mut Repository,
    record: &ma2a_core::SignedSpaceAddressRecordV1,
    now_ms: u64,
) -> TestResult {
    let chain = repository
        .load_space_chain(record.record().space_id())?
        .ok_or("address Space chain missing")?;
    let authorization = SpaceAuthorizationView::from_chain(&chain);
    let target =
        AddressRecordTarget::new(record.record().space_id(), record.record().endpoint_id());
    AddressRecordValidator::validate_and_store(
        repository,
        record.canonical_bytes(),
        target.validation(&authorization, now_ms),
    )?;
    Ok(())
}
