use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlPageV1, ControlResponseV1, EndpointId, MemberCapabilities,
    SpaceAddressRecordV1, SpaceAuthorizationView, SpaceId, SpaceManifestMembership, SpaceMemberV1,
    default_member_label,
};
use ma2a_net::{EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, StoreConfig,
    ValidatedAddressRecord,
};
use tokio::sync::mpsc;

use crate::{Runtime, RuntimeClock, RuntimeError};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct ManualClock(AtomicI64);

impl ManualClock {
    const fn new(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl RuntimeClock for ManualClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-clock-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test control clock state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

fn sign_address_response(
    (space_id, peer_key): (SpaceId, &SecretKey),
    data: AddressEndpointDataV1,
    validity: AddressRecordValidity,
) -> TestResult<Vec<u8>> {
    let peer_id = EndpointId::from(peer_key.public());
    let signed =
        SpaceAddressRecordV1::new(AddressRecordScope::new(space_id, peer_id), validity, data)
            .sign(peer_key)?;
    let artifact = ControlArtifactV1::new(
        ControlArtifactKind::ADDRESS_RECORD,
        signed.canonical_bytes().to_vec(),
    )?;
    let page = ControlPageV1::new(space_id, false, vec![artifact])?;
    Ok(ControlResponseV1::new(vec![page])?.encode()?)
}

fn seed_peer_address(
    config: &StoreConfig,
    (local_id, space_id): (EndpointId, SpaceId),
    (peer_key, peer_data): (&SecretKey, AddressEndpointDataV1),
) -> TestResult {
    let peer_id = EndpointId::from(peer_key.public());
    let initial = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(1, 10_000, 610_000)?,
        peer_data,
    )
    .sign(peer_key)?;
    let mut repository = Repository::open(config)?;
    let shared = repository
        .control_spaces_for(local_id)?
        .into_iter()
        .find(|space| space.chain().space_id() == space_id)
        .ok_or("shared space missing")?;
    let authorization = SpaceAuthorizationView::from_chain(shared.chain());
    let validated = ValidatedAddressRecord::parse(
        initial.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(space_id, peer_id),
            &authorization,
            10_000,
        ),
    )?;
    repository.advance_validated_address(&validated)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_round_accepts_peer_record_issued_after_round_start() -> TestResult {
    // Given: a Runtime and peer at T = 10_000 ms sharing one Space.
    let dir = TempState::new("fresh-response-clock")?;
    let config = StoreConfig::new(&dir.0);
    let clock = Arc::new(ManualClock::new(10_000));
    let runtime_clock = Arc::clone(&clock);
    let local = Runtime::start_with_clock(config.clone(), runtime_clock).await?;
    let local_id = local.handle().status().await?.endpoint_id();

    let peer_seed = [0x37; 32];
    let peer_key = SecretKey::from_bytes(&peer_seed);
    let peer_secret = EndpointSecret::parse(&peer_seed)?;
    let peer_id = peer_secret.endpoint_id();
    let (enroll_tx, _enroll_rx) = mpsc::channel(1);
    let (control_tx, mut control_rx) = mpsc::channel(8);
    let peer_endpoint = RuntimeEndpoint::bind_with_lookup(
        peer_secret,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(enroll_tx, None).with_control(control_tx, true),
    )
    .await?;
    let peer_data = peer_endpoint.endpoint_data()?;

    let space_id = local
        .handle()
        .create_owned_space("clock-round".to_owned())
        .await?;
    let caps = MemberCapabilities::new(true, true);
    let mut members = vec![
        SpaceMemberV1::new(local_id, default_member_label(local_id), caps)?,
        SpaceMemberV1::new(peer_id, "peer".to_owned(), caps)?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    local
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space_id,
            10_000,
            SpaceManifestMembership::new(members, Vec::new()),
        ))
        .await?;
    seed_peer_address(
        &config,
        (local_id, space_id),
        (&peer_key, peer_data.clone()),
    )?;

    let armed = Arc::new(AtomicBool::new(false));
    let peer_task = tokio::spawn({
        let armed = Arc::clone(&armed);
        let clock = Arc::clone(&clock);
        let peer_data = peer_data.clone();
        async move {
            while let Some(call) = control_rx.recv().await {
                let Some(call) = call.authorize() else {
                    continue;
                };
                let Ok((_request, responder)) = call.request().await else {
                    continue;
                };
                let response = if armed.swap(false, Ordering::SeqCst) {
                    clock.set(10_500);
                    sign_address_response(
                        (space_id, &peer_key),
                        peer_data.clone(),
                        AddressRecordValidity::new(2, 10_500, 610_500)?,
                    )?
                } else {
                    let empty = ControlPageV1::new(space_id, false, Vec::new())?;
                    ControlResponseV1::new(vec![empty])?.encode()?
                };
                responder.respond(Ok(response));
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        }
    });

    // Warm lookup cache and verify baseline connectivity at T = 10_000 ms.
    local.handle().observe_memberships(vec![space_id]).await?;
    local.handle().sync_control_with(peer_id).await?;

    // When: the next round starts at T = 10_000 ms, and while it is in flight the clock
    // advances to T + 500 ms and the peer signs a fresh sequence-2 address record at T + 500 ms.
    armed.store(true, Ordering::SeqCst);
    let sync_result = local.handle().sync_control_with(peer_id).await;

    // Then: the round validates the response against the fresh clock and persists sequence 2.
    assert!(
        sync_result.is_ok(),
        "control round rejected peer record issued during the round: {sync_result:?}"
    );
    let persisted = Repository::open(&config)?
        .address_record(space_id, peer_id)?
        .ok_or("persisted peer address record missing")?;
    assert_eq!(persisted.sequence(), 2);
    assert_eq!(persisted.issued_at_ms(), 10_500);

    drop(peer_endpoint);
    peer_task.abort();
    local.shutdown().await?;
    Ok(())
}
