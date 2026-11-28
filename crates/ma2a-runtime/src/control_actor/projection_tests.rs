use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::{MemberCapabilities, SpaceMemberV1, SpacePolicyV1};
use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{Repository, SpaceCreation, StoreConfig, StoreError};
use tokio::sync::mpsc;

use crate::{
    RuntimeError,
    actor::{
        Actor, RuntimeHandle,
        maintenance::{ControlRoundGate, FaultPoint},
    },
    control_sync::{ControlChanges, ControlRoundOutcome, ControlRoundScope, ControlRoundTrigger},
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-projection-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test control projection state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

async fn fixture(
    config: &StoreConfig,
) -> TestResult<(Actor, RuntimeHandle, tokio::task::JoinHandle<()>)> {
    let backend = StoreBackend::open(config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let lookup = SpaceAddressLookup::default();
    let (enroll_tx, enroll_rx) = mpsc::channel(1);
    let (ctrl_tx, ctrl_rx) = mpsc::channel(1);
    let (echo_tx, echo_rx) = mpsc::channel(1);
    let map = store.load_relay_map(identity.endpoint_id, 100).await?;
    let opts = EndpointBindOptions::new(enroll_tx, None)
        .with_control(ctrl_tx, false)
        .with_echo(echo_tx)
        .with_relay_map(map.clone());
    let endpoint = RuntimeEndpoint::bind_with_lookup(identity.secret, lookup.clone(), opts).await?;
    let metrics = endpoint.echo_metrics();
    let state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x61; 16],
        revision: store.revision().await?,
        memberships: BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let (actor, handle, _) = Actor::new(
        state,
        endpoint,
        store,
        enroll_rx,
        ctrl_rx,
        echo_rx,
        metrics,
        lookup,
        Arc::new(crate::clock::SystemClock),
        None,
    );
    Ok((actor, handle, store_task))
}

#[tokio::test]
async fn transient_inline_reconciliation_failure_schedules_control_projection() -> TestResult {
    let directory = TempState::new("inline-retry")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, _handle, store_task) = fixture(&config).await?;
    let member = SpaceMemberV1::new(
        actor.state.endpoint_id,
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = Repository::open(&config)?.create_owned_space(&SpaceCreation::new(
        10,
        member,
        SpacePolicyV1::phase_one_default(),
    ))?;

    let scheduled = actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no control round")?;
    actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let outcome = ControlRoundOutcome {
        revision: Some(created.revision()),
        dialed_peers: BTreeSet::new(),
        synchronized_peers: BTreeSet::new(),
        changes: ControlChanges {
            manifest: true,
            address: false,
            relay: false,
        },
        incomplete: false,
    };
    actor
        .finish_control_round(Ok((scheduled, Ok(Some(outcome)))))
        .await?;

    assert!(actor.maintenance.control_completion.is_some());
    assert!(
        !actor.maintenance.control_projections.is_empty(),
        "succeeded round whose inline reconciliation failed transiently must schedule control projection"
    );

    let wakeup = actor
        .maintenance
        .control_projections
        .join_next()
        .await
        .ok_or("missing control projection wakeup")?;
    wakeup?;
    actor.reconcile_control_completion().await?;
    assert!(actor.maintenance.control_completion.is_none());
    let triggers = actor
        .control_schedule_events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(triggers.contains(&ControlRoundTrigger::ManifestAdvanced));

    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}

#[tokio::test]
async fn transient_retry_failure_reschedules_control_projection() -> TestResult {
    let directory = TempState::new("retry-reschedule")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, handle, store_task) = fixture(&config).await?;
    let member = SpaceMemberV1::new(
        actor.state.endpoint_id,
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = Repository::open(&config)?.create_owned_space(&SpaceCreation::new(
        10,
        member,
        SpacePolicyV1::phase_one_default(),
    ))?;

    let scheduled = actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no control round")?;
    actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let outcome = ControlRoundOutcome {
        revision: Some(created.revision()),
        dialed_peers: BTreeSet::new(),
        synchronized_peers: BTreeSet::new(),
        changes: ControlChanges {
            manifest: true,
            address: false,
            relay: false,
        },
        incomplete: false,
    };
    actor
        .finish_control_round(Ok((scheduled, Ok(Some(outcome)))))
        .await?;
    assert_eq!(handle.candidate_refresh_attempts(), 1);
    assert!(actor.maintenance.control_completion.is_some());

    // Fail the first scheduled retry too, and hold Startup's control round so
    // only a rescheduled control projection can complete reconciliation.
    actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let (entered_tx, _entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *actor
        .maintenance
        .control_round_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ControlRoundGate {
        entered: entered_tx,
        release: release_rx,
    });
    let events = Arc::clone(&actor.control_schedule_events);
    let actor_task = tokio::spawn(actor.run());

    let observed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let scheduled_manifest = events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&ControlRoundTrigger::ManifestAdvanced);
            if handle.candidate_refresh_attempts() >= 3 && scheduled_manifest {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;

    let _released = release_tx.send(());
    handle.shutdown().await?;
    actor_task.await??;
    store_task.await?;

    let attempts = handle.candidate_refresh_attempts();
    let triggers = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        observed.is_ok(),
        "transient failure on control_projections retry must reschedule projection (attempts={attempts}, triggers={triggers:?})"
    );
    Ok(())
}
