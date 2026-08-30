use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use iroh::{
    Endpoint, RelayMode, SecretKey, address_lookup::memory::MemoryLookup, endpoint::presets,
};
use ma2a_core::{
    EchoError, MAX_WIRE_LEN, MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
};
use ma2a_net::{ECHO_ALPN, EndpointAddr};
use ma2a_runtime::{Runtime, RuntimeHandle};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-echo-adversarial-{label}-{}-{serial}",
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

pub(super) struct AuthorizedFixture {
    _state: TempState,
    runtime: Runtime,
    client: Endpoint,
    owner_addr: EndpointAddr,
    owner_id: ma2a_core::EndpointId,
    owner_member: SpaceMemberV1,
    peer_id: ma2a_core::EndpointId,
    space_id: SpaceId,
}

impl AuthorizedFixture {
    pub(super) async fn new(label: &str) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let config = StoreConfig::new(state.path());
        let runtime = Runtime::start(config.clone()).await?;
        let status = runtime.handle().status().await?;
        runtime.shutdown().await?;
        let peer_secret = SecretKey::generate();
        let peer_id = peer_secret.public().into();
        let owner_member = SpaceMemberV1::new(
            status.endpoint_id(),
            "owner".to_owned(),
            MemberCapabilities::new(true, true),
        )?;
        let peer_member = SpaceMemberV1::new(
            peer_id,
            "peer".to_owned(),
            MemberCapabilities::new(true, true),
        )?;
        let mut repository = Repository::open(&config)?;
        let space = repository.create_owned_space(&SpaceCreation::new(
            1,
            owner_member.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?;
        let mut members = vec![owner_member.clone(), peer_member];
        members.sort_by_key(SpaceMemberV1::endpoint_id);
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            space.space_id(),
            2,
            SpaceManifestMembership::new(members, vec![]),
        ))?;
        drop(repository);
        let runtime = Runtime::start(config).await?;
        let owner_addr = runtime.handle().status().await?.endpoint_addr().clone();
        let client = Endpoint::builder(presets::Minimal)
            .secret_key(peer_secret)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .address_lookup(MemoryLookup::with_provenance("ma2a_echo_adversarial"))
            .bind()
            .await?;
        Ok(Self {
            _state: state,
            runtime,
            client,
            owner_addr,
            owner_id: status.endpoint_id(),
            owner_member,
            peer_id,
            space_id: space.space_id(),
        })
    }

    pub(super) fn handle(&self) -> RuntimeHandle {
        self.runtime.handle()
    }

    async fn exchange(&self, body: &[u8]) -> TestResult<[u8; 37]> {
        let connection = tokio::time::timeout(
            IO_TIMEOUT,
            self.client.connect(self.owner_addr.clone(), ECHO_ALPN),
        )
        .await??;
        let (mut send, mut receive) = connection.open_bi().await?;
        send.write_all(body).await?;
        send.finish()?;
        let mut header = [0_u8; 37];
        receive.read_exact(&mut header).await?;
        connection.close(0_u8.into(), b"");
        Ok(header)
    }

    pub(super) const fn owner_id(&self) -> ma2a_core::EndpointId {
        self.owner_id
    }

    pub(super) async fn connect(&self) -> TestResult<iroh::endpoint::Connection> {
        Ok(tokio::time::timeout(
            IO_TIMEOUT,
            self.client.connect(self.owner_addr.clone(), ECHO_ALPN),
        )
        .await??)
    }

    pub(super) async fn shutdown(self) -> TestResult {
        self.client.close().await;
        self.runtime.shutdown().await?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_peer_is_denied_before_malformed_body_read() -> TestResult {
    // Given
    let fixture = AuthorizedFixture::new("revoked").await?;
    let before = fixture.handle().echo_metrics();
    fixture
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            fixture.space_id,
            3,
            SpaceManifestMembership::new(
                vec![fixture.owner_member.clone()],
                vec![SpaceRevocationV1::new(fixture.peer_id)],
            ),
        ))
        .await?;

    // When
    let header = fixture.exchange(&[0xff]).await?;

    // Then
    assert_eq!(header[0], EchoError::Unauthorized.code());
    assert_eq!(fixture.handle().echo_metrics(), before);
    assert!(fixture.handle().echo_audit().is_empty());
    fixture.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorized_malformed_body_returns_invalid_input() -> TestResult {
    // Given
    let fixture = AuthorizedFixture::new("malformed").await?;

    // When
    let header = fixture.exchange(&[0xff]).await?;

    // Then
    assert_eq!(header[0], EchoError::InvalidInput.code());
    assert_eq!(fixture.handle().echo_metrics().body_reads, 1);
    assert_eq!(fixture.handle().echo_metrics().decoded_requests, 1);
    assert!(fixture.handle().echo_audit().is_empty());
    fixture.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorized_oversized_body_is_rejected_before_decode() -> TestResult {
    // Given
    let fixture = AuthorizedFixture::new("oversized").await?;
    let body = vec![0_u8; MAX_WIRE_LEN + 1];

    // When
    let header = fixture.exchange(&body).await?;

    // Then
    assert_eq!(header[0], EchoError::InvalidInput.code());
    assert_eq!(fixture.handle().echo_metrics().body_reads, 0);
    assert_eq!(fixture.handle().echo_metrics().decoded_requests, 0);
    assert!(fixture.handle().echo_audit().is_empty());
    fixture.shutdown().await
}
