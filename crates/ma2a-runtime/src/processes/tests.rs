//! The Process manager against a real Store, real sockets, and in-process supervisors.
#![cfg(unix)]

mod driver;
mod recovery;

use std::{path::PathBuf, sync::Arc, time::Duration};

use ma2a_core::{
    EndpointId, RequestId,
    service::{ProcessId, ProcessSpec, ProcessStart, ProcessState},
};
use ma2a_store::{ProcessCreation, ProcessRecord, StoreConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ProcessHosting, ProcessManager};
use crate::{
    clock::SystemClock,
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MIB: u32 = 1_048_576;

/// A private state directory removed on drop.
struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let nonce = u64::from_le_bytes(nonce);
        let path = std::env::temp_dir().join(format!("ma2a-pm-{label}-{nonce:016x}"));
        std::fs::create_dir(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    state: TempState,
    store: StoreClient,
    local: EndpointId,
    creator: EndpointId,
}

impl Harness {
    async fn new(label: &str) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let creator = iroh::SecretKey::generate().public().into();
        Ok(Self {
            state,
            store,
            local,
            creator,
        })
    }

    /// A manager as one daemon incarnation would run it; cancel the token to end it.
    fn manager(&self) -> (ProcessManager, CancellationToken) {
        let shutdown = CancellationToken::new();
        let manager = ProcessManager::new(
            (self.local, self.store.clone(), Arc::new(SystemClock)),
            (&self.state.0, Some(ProcessHosting::in_process())),
            shutdown.clone(),
        );
        (manager, shutdown)
    }
}

fn start(script: &str) -> TestResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?;
    Ok(ProcessStart::new(spec, false, MIB)?)
}

fn request_id(byte: u8) -> TestResult<RequestId> {
    Ok(RequestId::try_from([byte; 16].as_slice())?)
}

/// Creates and brings up one Process as a remote `start` would.
async fn started(
    harness: &Harness,
    manager: &ProcessManager,
    (byte, script): (u8, &str),
) -> TestResult<ProcessRecord> {
    let start = start(script)?;
    let binding = (harness.creator, request_id(byte)?);
    let (creation, credentials) = manager.create(binding, &start).await?;
    let ProcessCreation::Created(record) = creation else {
        return Err("expected a new Process".into());
    };
    assert_eq!(record.state, ProcessState::Created);
    Ok(manager.bring_up(credentials, start).await?)
}

/// Waits until one Process reaches `state`.
async fn reaches(
    manager: &ProcessManager,
    id: ProcessId,
    state: ProcessState,
) -> TestResult<ProcessRecord> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let record = manager.record(id).await?.ok_or("missing Process")?;
        if record.state == state {
            return Ok(record);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "stuck in {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_started_process_runs_and_its_exit_is_recorded() -> TestResult {
    let harness = Harness::new("exit").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (1, "echo hi; sleep 0.2; exit 4")).await?;
    assert!(
        matches!(record.state, ProcessState::Running | ProcessState::Exited),
        "{:?}",
        record.state
    );
    let ended = reaches(&manager, record.id, ProcessState::Exited).await?;
    assert_eq!(
        ended.termination,
        Some(ma2a_core::service::Termination::Exited(4))
    );
    assert!(ended.secret.is_none(), "the secret is gone once terminal");
    assert_eq!(manager.window(record.id).await.end, 3);
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_id_binds_exactly_one_process() -> TestResult {
    let harness = Harness::new("binding").await?;
    let (manager, shutdown) = harness.manager();
    let first = started(&harness, &manager, (2, "sleep 60")).await?;
    let binding = (harness.creator, request_id(2)?);
    let (again, _) = manager.create(binding, &start("sleep 60")?).await?;
    let ProcessCreation::Existing(existing) = again else {
        return Err("a retry must answer the existing Process".into());
    };
    assert_eq!(existing.id, first.id);
    let (different, _) = manager.create(binding, &start("sleep 61")?).await?;
    assert!(matches!(different, ProcessCreation::Conflict));
    let target = (None, first.id);
    let ended = manager
        .terminate(
            target,
            (1_000, ma2a_core::service::ProcessReason::Requested),
        )
        .await?;
    assert_eq!(ended.state, ProcessState::Terminated);
    assert_eq!(
        ended.reason,
        Some(ma2a_core::service::ProcessReason::Requested)
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_daemon_readopts_a_running_process_through_the_proof() -> TestResult {
    let harness = Harness::new("readopt").await?;
    let (first, first_shutdown) = harness.manager();
    let record = started(&harness, &first, (3, "sleep 60")).await?;
    reaches(&first, record.id, ProcessState::Running).await?;
    // The daemon goes away; its supervisor and child keep running.
    first_shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (second, second_shutdown) = harness.manager();
    second.reconcile().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = reaches(&second, record.id, ProcessState::Running).await?;
    assert_eq!(still.times.started_ms, record.times.started_ms);
    let ended = second
        .terminate(
            (None, record.id),
            (1_000, ma2a_core::service::ProcessReason::Requested),
        )
        .await?;
    assert_eq!(ended.state, ProcessState::Terminated);
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawn_authorization_terminates_the_creators_processes() -> TestResult {
    let harness = Harness::new("withdrawn").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (4, "sleep 60")).await?;
    reaches(&manager, record.id, ProcessState::Running).await?;
    // This creator shares no Space with this Runtime, so it holds no authorization path.
    manager.converge().await;
    let ended = reaches(&manager, record.id, ProcessState::Terminated).await?;
    assert_eq!(
        ended.reason,
        Some(ma2a_core::service::ProcessReason::Unauthorized)
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_starts_make_room_by_archiving_the_oldest_ended_local_process() -> TestResult {
    let harness = Harness::new("local-room").await?;
    let (manager, shutdown) = harness.manager();
    let retained = u8::try_from(ma2a_store::MAX_RETAINED_PROCESSES_PER_CREATOR)?;
    let mut ended = Vec::new();
    for byte in 1..=retained {
        let record = manager
            .start_local(request_id(byte)?, start("exit 0")?)
            .await?;
        ended.push(reaches(&manager, record.id, ProcessState::Exited).await?.id);
    }

    // Every retained slot holds an ended instance, yet the next one still starts.
    let next = manager
        .start_local(request_id(retained + 1)?, start("exit 0")?)
        .await?;
    let (first, rest) = ended.split_first().ok_or("no Process")?;
    let oldest = manager.local_record(*first).await?.ok_or("no record")?;
    assert!(oldest.times.archived_ms.is_some(), "{oldest:?}");
    for kept in rest {
        let kept = manager.local_record(*kept).await?.ok_or("no record")?;
        assert!(kept.times.archived_ms.is_none(), "{kept:?}");
    }
    reaches(&manager, next.id, ProcessState::Exited).await?;
    shutdown.cancel();
    Ok(())
}
