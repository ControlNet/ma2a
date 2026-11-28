use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlPageV1, ControlResponseV1, EndpointId, MemberCapabilities,
    SpaceAddressRecordV1, SpaceAuthorizationView, SpaceChain, SpaceId, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1, default_member_label,
};
use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, SpaceCreation,
    StoreConfig, ValidatedAddressRecord,
};
use tokio::sync::mpsc;

use crate::{
    RuntimeClock, RuntimeError,
    actor::Actor,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FixedClock(i64);

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0)
    }
}

#[derive(Debug)]
pub(super) struct FailSecondClock {
    calls: AtomicUsize,
    error: fn() -> RuntimeError,
}

impl FailSecondClock {
    pub(super) const fn new(error: fn() -> RuntimeError) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            error,
        }
    }
}

impl RuntimeClock for FailSecondClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(10_000)
        } else {
            Err((self.error)())
        }
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-deadline-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test control deadline state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Fixture {
    _local_dir: TempState,
    _owner_dir: TempState,
    pub(super) actor: Actor,
    pub(super) store_task: tokio::task::JoinHandle<()>,
    pub(super) space_id: SpaceId,
    pub(super) owner_id: EndpointId,
    pub(super) peer_b_id: EndpointId,
    pub(super) gen2_response: Vec<u8>,
    pub(super) empty_response: Vec<u8>,
}

fn seed_address(
    repo: &mut Repository,
    (space_id, chain): (SpaceId, &SpaceChain),
    peer_key: &SecretKey,
) -> TestResult {
    let peer_id = EndpointId::from(peer_key.public());
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(1, 10_000, 610_000)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(peer_key)?;
    let auth = SpaceAuthorizationView::from_chain(chain);
    let validated = ValidatedAddressRecord::parse(
        signed.canonical_bytes(),
        AddressRecordValidation::new(AddressRecordTarget::new(space_id, peer_id), &auth, 10_000),
    )?;
    repo.advance_validated_address(&validated)?;
    Ok(())
}

async fn build_actor(
    local_cfg: &StoreConfig,
    seed: u8,
) -> TestResult<(Actor, tokio::task::JoinHandle<()>)> {
    let backend = StoreBackend::open(local_cfg)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let lookup = SpaceAddressLookup::default();
    let (enroll_tx, enroll_rx) = mpsc::channel(1);
    let (ctrl_tx, ctrl_rx) = mpsc::channel(1);
    let (echo_tx, echo_rx) = mpsc::channel(1);
    let map = store.load_relay_map(identity.endpoint_id, 10_000).await?;
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enroll_tx, None)
            .with_control(ctrl_tx, false)
            .with_echo(echo_tx)
            .with_relay_map(map.clone()),
    )
    .await?;
    let metrics = endpoint.echo_metrics();
    let state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [seed; 16],
        revision: store.revision().await?,
        memberships: BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let (actor, _, _) = Actor::new(
        state,
        endpoint,
        store,
        enroll_rx,
        ctrl_rx,
        echo_rx,
        metrics,
        lookup,
        Arc::new(FixedClock(10_000)),
        None,
    );
    Ok((actor, store_task))
}

pub(super) async fn setup_fixture(label: &str, seed: u8) -> TestResult<Fixture> {
    let local_dir = TempState::new(label)?;
    let local_cfg = StoreConfig::new(&local_dir.0);
    let (mut actor, store_task) = build_actor(&local_cfg, seed).await?;
    let local_id = actor.state.endpoint_id;
    let owner_key = SecretKey::from_bytes(&[seed; 32]);
    let peer_b_key = SecretKey::from_bytes(&[seed.wrapping_add(1); 32]);
    let owner_id = EndpointId::from(owner_key.public());
    let peer_b_id = EndpointId::from(peer_b_key.public());
    let owner_dir = TempState::new(&format!("{label}-owner"))?;
    let mut owner_repo = Repository::open(&StoreConfig::new(&owner_dir.0))?;
    let caps = MemberCapabilities::new(true, true);
    let owner_m = SpaceMemberV1::new(owner_id, default_member_label(owner_id), caps)?;
    let local_m = SpaceMemberV1::new(local_id, default_member_label(local_id), caps)?;
    let peer_b_m = SpaceMemberV1::new(peer_b_id, "peer-b".to_owned(), caps)?;
    let space_id = owner_repo
        .create_owned_space(&SpaceCreation::new(
            10_000,
            owner_m.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    let mut members1 = vec![owner_m.clone(), local_m.clone(), peer_b_m];
    members1.sort_by_key(SpaceMemberV1::endpoint_id);
    let gen1 = owner_repo.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        10_000,
        SpaceManifestMembership::new(members1, Vec::new()),
    ))?;
    let mut members2 = vec![owner_m, local_m];
    members2.sort_by_key(SpaceMemberV1::endpoint_id);
    let gen2 = owner_repo.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        10_000,
        SpaceManifestMembership::new(members2, vec![SpaceRevocationV1::new(peer_b_id)]),
    ))?;
    let manifest2 = gen2
        .chain()
        .manifests()
        .last()
        .ok_or("gen 2 manifest missing")?
        .canonical_bytes()
        .to_vec();
    let gen2_response = ControlResponseV1::new(vec![ControlPageV1::new(
        space_id,
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::MANIFEST,
            manifest2,
        )?],
    )?])?
    .encode()?;
    let empty_response =
        ControlResponseV1::new(vec![ControlPageV1::new(space_id, false, Vec::new())?])?.encode()?;

    let mut local_repo = Repository::open(&local_cfg)?;
    local_repo.persist_space_chain(gen1.chain())?;
    seed_address(&mut local_repo, (space_id, gen1.chain()), &owner_key)?;
    seed_address(&mut local_repo, (space_id, gen1.chain()), &peer_b_key)?;
    let seeded_rev = local_repo.revision()?;
    drop(local_repo);

    actor.state.revision = actor.state.revision.max(seeded_rev);
    actor.state.memberships.insert(space_id);
    actor.synchronized_control_peers.extend([peer_b_id]);
    assert!(actor.synchronized_control_peers.contains(&peer_b_id));
    Ok(Fixture {
        _local_dir: local_dir,
        _owner_dir: owner_dir,
        actor,
        store_task,
        space_id,
        owner_id,
        peer_b_id,
        gen2_response,
        empty_response,
    })
}
