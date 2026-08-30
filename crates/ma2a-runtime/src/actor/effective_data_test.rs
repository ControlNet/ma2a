use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::{
    MemberCapabilities, SignedSpaceAddressRecordV1, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_net::{
    EndpointBindOptions, LocalIrohRelayMap, RuntimeEndpoint, SpaceAddressLookup, UserData,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};
use tokio::sync::mpsc;

use super::Actor;
use crate::{
    RuntimeClock,
    error::RuntimeError,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

#[path = "relay_reachability_test.rs"]
mod relay_reachability_test;
#[path = "relay_refresh_test.rs"]
mod relay_refresh_test;

type TestError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), TestError>;
const NOW_MS: i64 = 1_700_000_000_000;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(NOW_MS)
    }
}

struct TempState(PathBuf);

impl TempState {
    fn new() -> Result<Self, TestError> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-effective-data-{}-{serial}",
            std::process::id(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[expect(
    clippy::too_many_lines,
    reason = "the single end-to-end scenario keeps setup, mutation, and persisted proofs visible"
)]
async fn actor_publishes_exact_next_record_to_every_space_for_user_data_only_change() -> TestResult
{
    // Given
    let state_dir = TempState::new()?;
    let config = StoreConfig::new(state_dir.path());
    let backend = StoreBackend::open(&config)?;
    let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(store_receiver));
    let store = StoreClient::new(store_sender);
    let identity = store.initialize().await?;
    let mut repository = Repository::open(&config)?;
    let spaces = [
        create_space(&mut repository, identity.endpoint_id, 10)?,
        create_space(&mut repository, identity.endpoint_id, 20)?,
    ];
    let memberships = repository.memberships_for(identity.endpoint_id)?;
    let relay_map = LocalIrohRelayMap::from_control_spaces(
        &repository.control_spaces_for(identity.endpoint_id)?,
        None,
        u64::try_from(NOW_MS)?,
    );
    let lookup = SpaceAddressLookup::default();
    let (enrollment_sender, enrollment_calls) = mpsc::channel(1);
    let (control_sender, control_calls) = mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enrollment_sender, identity.bind_port)
            .with_control(control_sender, true)
            .with_relay_map(relay_map.clone()),
    )
    .await?;
    let endpoint_data = endpoint.endpoint_data()?;
    let runtime_state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data,
        boot_id: [0x42; 16],
        revision: repository.revision()?,
        memberships,
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    };
    let (mut actor, _handle, _cancellation) = Actor::new(
        runtime_state,
        endpoint,
        store,
        enrollment_calls,
        control_calls,
        lookup,
        Arc::new(FixedClock),
    );
    let _initial_observation = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        actor.relay_observations.recv(),
    )
    .await?
    .ok_or("initial relay observation channel closed")?;
    let publisher = actor.endpoint.address_publisher()?;
    let (_revision, advanced) = actor
        .store
        .publish_address(
            publisher,
            identity.endpoint_id,
            u64::try_from(NOW_MS)?,
            false,
        )
        .await?;
    assert!(advanced);
    let initial = spaces
        .iter()
        .map(|space_id| {
            repository
                .address_record(*space_id, identity.endpoint_id)?
                .ok_or_else(|| "initial address record missing".into())
        })
        .collect::<Result<Vec<_>, Box<dyn Error + Send + Sync>>>()?;

    // When
    actor
        .endpoint
        .set_user_data_for_address_lookup(Some(UserData::try_from(
            "actor-effective-metadata".to_owned(),
        )?));
    let observation = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        actor.relay_observations.recv(),
    )
    .await?
    .ok_or("updated relay observation channel closed")?;
    actor.observe_iroh_relay(observation).await?;

    // Then
    for (space_id, previous) in spaces.iter().zip(initial) {
        let current = repository
            .address_record(*space_id, identity.endpoint_id)?
            .ok_or("updated address record missing")?;
        let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(current.signed_record())?;
        assert_eq!(current.endpoint_id(), identity.endpoint_id);
        assert_eq!(current.sequence(), previous.sequence() + 1);
        assert_eq!(
            signed.record().endpoint_data().user_data(),
            Some("actor-effective-metadata")
        );
    }
    actor.control_rounds.shutdown().await;
    let Actor {
        endpoint,
        store,
        relay_observer,
        ..
    } = actor;
    endpoint.shutdown().await?;
    relay_observer.await?;
    store.stop().await?;
    store_task.await?;
    Ok(())
}

fn create_space(
    repository: &mut Repository,
    endpoint_id: ma2a_core::EndpointId,
    issued_at_ms: u64,
) -> Result<ma2a_core::SpaceId, Box<dyn Error + Send + Sync>> {
    let member = SpaceMemberV1::new(
        endpoint_id,
        format!("local-{issued_at_ms}"),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        issued_at_ms,
        member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        issued_at_ms + 1,
        SpaceManifestMembership::new(vec![member], Vec::new()),
    ))?;
    Ok(created.space_id())
}
