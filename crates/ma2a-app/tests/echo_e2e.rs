//! Real-Iroh Echo v1 authorization and round-trip integration tests.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use iroh::{Endpoint, RelayMode, address_lookup::memory::MemoryLookup, endpoint::presets};
use ma2a_core::{
    EchoError, EchoRequest, InviteEntropy, MemberCapabilities, RequestId, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_net::ECHO_ALPN;
use ma2a_runtime::{EnrollmentAttempt, EnrollmentCreation, Runtime};
use ma2a_store::{Repository, SpaceCreation, StoreConfig};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-echo-{label}-{}-{serial}", std::process::id()));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_space_peer_receives_exact_echo_over_real_iroh() -> TestResult {
    // Given
    let owner_state = TempState::new("owner")?;
    let candidate_state = TempState::new("candidate")?;
    let owner_config = StoreConfig::new(owner_state.path());
    let owner = Runtime::start(owner_config.clone()).await?;
    let owner_status = owner.handle().status().await?;
    owner.shutdown().await?;
    let mut repository = Repository::open(&owner_config)?;
    let owner_member = SpaceMemberV1::new(
        owner_status.endpoint_id(),
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let space = repository.create_owned_space(&SpaceCreation::new(
        1,
        owner_member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);
    let owner = Runtime::start(owner_config).await?;
    let candidate = Runtime::start(StoreConfig::new(candidate_state.path())).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space.space_id(),
            300_000,
            InviteEntropy::from_bytes([0x31; 16], [0x41; 32]),
        )?)
        .await?;
    candidate
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x51; 16].as_slice())?,
            "candidate".to_owned(),
        ))
        .await?;
    let payload = b"encrypted echo";

    // When
    let response = candidate
        .handle()
        .echo(
            RequestId::try_from([0x61; 16].as_slice())?,
            owner_status.endpoint_id(),
            payload,
        )
        .await?;

    // Then
    assert_eq!(response.responder_endpoint_id(), owner_status.endpoint_id());
    assert_eq!(response.payload(), payload);
    assert_eq!(owner.handle().echo_metrics().body_reads, 1);
    assert_eq!(owner.handle().echo_metrics().decoded_requests, 1);
    assert_eq!(owner.handle().echo_audit().len(), 1);
    assert_eq!(candidate.handle().echo_audit().len(), 1);
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_zero_space_peer_is_denied_before_echo_body_read() -> TestResult {
    // Given
    let state = TempState::new("isolated")?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    let status = runtime.handle().status().await?;
    runtime.shutdown().await?;
    let mut repository = Repository::open(&config)?;
    repository.create_owned_space(&SpaceCreation::new(
        1,
        SpaceMemberV1::new(
            status.endpoint_id(),
            "isolated".to_owned(),
            MemberCapabilities::new(true, true),
        )?,
        SpacePolicyV1::phase_one_default(),
    ))?;
    drop(repository);
    let runtime = Runtime::start(config).await?;
    let status = runtime.handle().status().await?;
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_echo_e2e"))
        .bind()
        .await?;
    let request = EchoRequest::new(
        RequestId::try_from([0x71; 16].as_slice())?,
        status.endpoint_id(),
        b"must not be read",
    )?
    .encode()?;

    // When
    let connection = tokio::time::timeout(
        IO_TIMEOUT,
        client.connect(status.endpoint_addr(), ECHO_ALPN),
    )
    .await??;
    let (mut send, mut receive) = connection.open_bi().await?;
    send.write_all(&request).await?;
    send.finish()?;
    let mut header = [0_u8; 37];
    receive.read_exact(&mut header).await?;

    // Then
    assert_eq!(header[0], EchoError::Unauthorized.code());
    assert_eq!(runtime.handle().echo_metrics().body_reads, 0);
    assert_eq!(runtime.handle().echo_metrics().decoded_requests, 0);
    connection.close(0_u8.into(), b"");
    client.close().await;
    runtime.shutdown().await?;
    Ok(())
}
