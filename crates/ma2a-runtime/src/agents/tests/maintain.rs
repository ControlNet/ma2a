//! Maintenance keeps task cancellation expiry moving while slow Agent startups run separately.
#![cfg(unix)]

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskId, AgentTaskReason, AgentTaskState, MAX_LOG_LIMIT,
        MessageRole, ProcessId, ProcessSpec, ProcessStart, ProcessState,
    },
};
use ma2a_store::{
    AgentDefinition, AgentOutput, AgentOutputEffect, AgentTaskCreation, NewAgentTask, NewProcess,
    ProcessTransition, Repository, StoreConfig, TASK_ENDED_ANSWER,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    RuntimeClock,
    processes::{ProcessHosting, ProcessManager},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

/// Creates a private state directory with a short name: a supervisor's socket lives several
/// levels below it, and a Unix socket path holds at most 103 bytes. `label` names it in
/// diagnostics only.
fn short_state_dir(label: &str) -> TestResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;

    let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ma2a-{:x}-{serial}", std::process::id()));
    eprintln!("{label}: state directory {}", dir.display());
    std::fs::create_dir(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Clock tied to Tokio's paused time so `tokio::time::advance` drives `now_ms()`.
#[derive(Debug)]
struct TokioClock {
    epoch: tokio::time::Instant,
    base_ms: i64,
}

impl TokioClock {
    fn new(base_ms: i64) -> Self {
        Self {
            epoch: tokio::time::Instant::now(),
            base_ms,
        }
    }
}

impl RuntimeClock for TokioClock {
    fn now_ms(&self) -> Result<i64, crate::RuntimeError> {
        let elapsed = i64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(i64::MAX);
        Ok(self.base_ms.saturating_add(elapsed))
    }
}

fn seed_agents(config: &StoreConfig, local: EndpointId) -> TestResult<AgentTaskId> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    for name in ["alpha-1", "alpha-2"] {
        let id = AgentId::new(name)?;
        repository.define_agent(
            &AgentDefinition {
                id: id.clone(),
                description: String::new(),
                start: start_bytes.clone(),
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&id, (true, 2))?;
    }

    let beta = AgentId::new("beta")?;
    repository.define_agent(
        &AgentDefinition {
            id: beta.clone(),
            description: String::new(),
            start: start_bytes.clone(),
            tools: ma2a_agent::tools::ToolFamily::Agent.bit(),
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&beta, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&beta, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no beta instance")?
        .generation;
    let beta_pid = ProcessId::from_bytes([0x55; 16]);
    repository.create_process(&NewProcess {
        id: beta_pid,
        binding: (local, RequestId::random()?),
        fingerprint: [0x66; 32],
        start: (&start_bytes, None),
        secret: [0x77; 32],
        created_ms: 4,
    })?;
    for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
        repository.transition_process(
            beta_pid,
            ProcessTransition {
                to,
                reason: None,
                termination: None,
                at_ms,
            },
        )?;
    }
    repository.bind_agent_process((&beta, generation), beta_pid)?;
    repository.apply_agent_output(
        &AgentOutput {
            agent: beta.clone(),
            generation,
            cursor: 1,
            at_ms: 7,
        },
        AgentOutputEffect::Ready,
    )?;
    let task_id = AgentTaskId::random()?;
    let AgentTaskCreation::Created(task) = repository
        .create_agent_task(&NewAgentTask {
            id: task_id,
            agent: beta.clone(),
            binding: (local, RequestId::random()?),
            fingerprint: [0x88; 32],
            from_agent: None,
            input: AgentMessage::text(MessageRole::User, "work")?,
            created_ms: 8,
        })?
        .into_value()
    else {
        return Err("task was not created".into());
    };
    repository.assign_agent_tasks(&beta, 9)?;
    repository.apply_agent_output(
        &AgentOutput {
            agent: beta,
            generation,
            cursor: 2,
            at_ms: 10,
        },
        AgentOutputEffect::ToolCall {
            task: task.summary.id,
            call: "c1".to_owned(),
            name: "agent.get".to_owned(),
            request: b"{}".to_vec(),
            rejection: b"{\"error\":{\"code\":\"concurrency_exceeded\"}}".to_vec(),
        },
    )?;
    Ok(task.summary.id)
}

#[tokio::test(start_paused = true)]
async fn slow_agent_startups_do_not_delay_cancellation_expiry() -> TestResult {
    // Given: two enabled Agents whose supervisor binary never binds (each holding startup for 10 s),
    // and a third ready Agent with a running task and an open tool call.
    let dir = short_state_dir("ma2a-maintain")?;
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;

    let clock: Arc<dyn RuntimeClock> = Arc::new(TokioClock::new(1_000_000));
    let shutdown = CancellationToken::new();
    let hosting = ProcessHosting::executable(
        PathBuf::from("/bin/sh"),
        vec!["-c".into(), "cat >/dev/null".into()],
    );
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, Some(hosting)),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    let beta_task = seed_agents(&config, local)?;
    // Mark the tool call as currently running so reconcile's pending-call sweep leaves it open
    // until cancellation grace expires.
    manager
        .coordination()
        .calls
        .insert((beta_task, "c1".to_owned()));

    // When: cancellation is requested at t=0 and the maintenance loop runs for 12 seconds.
    manager
        .request_cancel((None, None), (beta_task, AgentTaskReason::Requested))
        .await?;
    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    for _ in 0..8 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
    }
    for _ in 0..12 {
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
        }
        // Flush the multi-step Store round trips of one maintenance tick.
        for _ in 0..8 {
            let _rev = store.revision().await?;
            tokio::task::yield_now().await;
        }
    }

    // Then: beta's task expired within the 10 s grace and its open tool call got `task_ended`,
    // while alpha-1 and alpha-2 ran their slow startup attempts independently.
    let task_record = Repository::open(&config)?.agent_task(beta_task)?;
    let tool_call = Repository::open(&config)?.tool_call((beta_task, "c1"))?;
    let alpha_1 = Repository::open(&config)?.agent(&AgentId::new("alpha-1")?)?;
    let alpha_2 = Repository::open(&config)?.agent(&AgentId::new("alpha-2")?)?;

    shutdown.cancel();
    for _ in 0..20 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
        if runner.is_finished() {
            break;
        }
    }
    let clean_shutdown = runner.is_finished();
    if !clean_shutdown {
        runner.abort();
    }
    let _joined = runner.await;
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    assert!(clean_shutdown, "maintenance should shut down promptly");
    for alpha in [alpha_1, alpha_2] {
        let record = alpha.ok_or("slow agent missing")?;
        assert!(
            record.generation >= 1,
            "slow agent never attempted startup: {record:?}"
        );
    }
    let task_record = task_record.ok_or("task missing")?;
    assert_eq!(
        task_record.summary.state,
        AgentTaskState::Cancelled,
        "task cancellation should expire within grace despite slow Agent startups"
    );
    assert_eq!(task_record.summary.reason, Some(AgentTaskReason::Requested));
    let tool_call = tool_call.ok_or("tool call missing")?;
    assert_eq!(
        tool_call.result.as_deref(),
        Some(TASK_ENDED_ANSWER),
        "open tool call should be answered task_ended on cancellation expiry"
    );
    Ok(())
}

type FailingAgent = (AgentId, ProcessId, [u8; 32]);

fn seed_failing_predecessor_agents(
    config: &StoreConfig,
    local: EndpointId,
    state: ProcessState,
) -> TestResult<Vec<FailingAgent>> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    let mut failing = Vec::with_capacity(8);
    for index in 1_u8..=8 {
        let id = AgentId::new(&format!("fail-{index}"))?;
        repository.define_agent(
            &AgentDefinition {
                id: id.clone(),
                description: String::new(),
                start: start_bytes.clone(),
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&id, (true, 2))?;
        let generation = repository
            .begin_agent_instance(&id, 3)?
            .into_value()
            .and_then(|record| record.instance)
            .ok_or("no failing instance")?
            .generation;
        let pid = ProcessId::from_bytes([0x90 + index; 16]);
        let secret = [0xA0 + index; 32];
        repository.create_process(&NewProcess {
            id: pid,
            binding: (local, RequestId::random()?),
            fingerprint: [0xB0 + index; 32],
            start: (&start_bytes, None),
            secret,
            created_ms: 4,
        })?;
        if state == ProcessState::Running {
            for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
                repository.transition_process(
                    pid,
                    ProcessTransition {
                        to,
                        reason: None,
                        termination: None,
                        at_ms,
                    },
                )?;
            }
        }
        repository.bind_agent_process((&id, generation), pid)?;
        repository.set_agent_enabled(&id, (false, 7))?;
        repository.set_agent_enabled(&id, (true, 8))?;
        failing.push((id, pid, secret));
    }
    let healthy = AgentId::new("healthy")?;
    repository.define_agent(
        &AgentDefinition {
            id: healthy.clone(),
            description: String::new(),
            start: start_bytes,
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&healthy, (true, 2))?;
    Ok(failing)
}

#[tokio::test(start_paused = true)]
async fn eight_failing_predecessor_recoveries_do_not_starve_ninth_healthy_agent() -> TestResult {
    // Given: eight enabled Agents (`fail-1`..`fail-8`) whose previous Processes cannot be
    // confirmed terminal, ahead in name order of a ninth healthy Agent (`healthy`).
    let dir = short_state_dir("ma2a-starve")?;
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;
    let failing = seed_failing_predecessor_agents(&config, local, ProcessState::Created)?;

    let clock: Arc<dyn RuntimeClock> = Arc::new(TokioClock::new(1_000_000));
    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, None),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    // When: maintenance runs for 5 ticks.
    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    for _ in 0..8 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
    }
    for _ in 0..5 {
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
        }
        for _ in 0..8 {
            let _rev = store.revision().await?;
            tokio::task::yield_now().await;
        }
    }

    let healthy = Repository::open(&config)?.agent(&AgentId::new("healthy")?)?;
    let backoffs: Vec<_> = {
        let coordination = manager.coordination();
        failing
            .iter()
            .map(|(id, _, _)| {
                (
                    id.clone(),
                    coordination.restarts.get(id).map(|(_, next)| *next),
                )
            })
            .collect()
    };

    shutdown.cancel();
    for _ in 0..20 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
        if runner.is_finished() {
            break;
        }
    }
    if !runner.is_finished() {
        runner.abort();
    }
    let _joined = runner.await;
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    // Then: `healthy` claimed a start slot and began its first instance, while each failing
    // predecessor recovery was backed off rather than hammering every tick.
    let healthy = healthy.ok_or("healthy agent missing")?;
    assert!(
        healthy.instance.is_some(),
        "healthy agent starved behind eight failing predecessor recoveries: {healthy:?}"
    );
    for (id, next_backoff) in backoffs {
        assert!(
            next_backoff.is_some_and(|next| next >= Duration::from_secs(2)),
            "failing predecessor recovery for {id} did not schedule retry backoff: {next_backoff:?}"
        );
    }
    Ok(())
}

async fn run_dropping_supervisor(
    (address, credentials): (
        ma2a_services::supervisor::SupervisorAddress,
        ma2a_services::supervisor::Credentials,
    ),
    (connected, mut release, shutdown): (
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::sync::watch::Receiver<bool>,
        CancellationToken,
    ),
) -> TestResult {
    use interprocess::local_socket::tokio::prelude::*;
    use ma2a_services::{
        FrameReader, FrameWriter,
        supervisor::proof::{prove_supervisor, read_challenge},
    };

    let listener = address.bind()?;
    loop {
        let mut stream = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => accepted?,
        };
        let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
        let (recv, send) = stream.split();
        let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
        prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
        let _request = reader.expect().await?;
        connected.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _released = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            res = release.wait_for(|open| *open) => res,
        };
        drop((reader, writer));
    }
}

fn spawn_dropping_supervisors(
    (sockets_dir, failing): (&std::path::Path, &[FailingAgent]),
    (connected, release, shutdown): (
        &Arc<std::sync::atomic::AtomicUsize>,
        &tokio::sync::watch::Receiver<bool>,
        &CancellationToken,
    ),
) -> Vec<tokio::task::JoinHandle<TestResult>> {
    use ma2a_services::supervisor::{Credentials, SupervisorAddress};

    failing
        .iter()
        .map(|(_, pid, secret)| {
            let address = SupervisorAddress::new(
                sockets_dir
                    .join(format!("{pid}.sock"))
                    .to_string_lossy()
                    .into_owned(),
            );
            tokio::spawn(run_dropping_supervisor(
                (address, Credentials::new(*pid, *secret)),
                (Arc::clone(connected), release.clone(), shutdown.clone()),
            ))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ninth_healthy_agent_claims_slot_when_failing_supervisor_releases_permit() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Given: eight enabled Agents (`fail-1`..`fail-8`) whose running predecessors have mock
    // supervisors that accept Terminate and drop without answering, plus a ninth healthy Agent.
    let dir = short_state_dir("ma2a-slot")?;
    let sockets_dir = dir.join("run-v1").join("p");
    std::fs::create_dir_all(&sockets_dir)?;
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;
    let failing = seed_failing_predecessor_agents(&config, local, ProcessState::Running)?;

    let shutdown = CancellationToken::new();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let connected = Arc::new(AtomicUsize::new(0));
    let servers = spawn_dropping_supervisors(
        (&sockets_dir, &failing),
        (&connected, &release_rx, &shutdown),
    );

    let clock: Arc<dyn RuntimeClock> = Arc::new(crate::clock::SystemClock);
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, None),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    // When: one reconciliation pass fills all 8 start permits with the 8 failing predecessor
    // recoveries (holding the periodic tick so no later 1 s tick can admit `healthy`), then the
    // supervisors drop their Terminate connections to free the permits.
    manager.reconcile().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while connected.load(Ordering::SeqCst) < 8 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "not all 8 failing recoveries reached their mock supervisors"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(manager.0.start_permits.available_permits(), 0);
    let healthy_id = AgentId::new("healthy")?;
    let before_release = Repository::open(&config)?
        .agent(&healthy_id)?
        .ok_or("healthy missing")?;
    assert_eq!(before_release.instance, None);

    let _sent = release_tx.send(true);
    let healthy_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut healthy_started = false;
    while tokio::time::Instant::now() < healthy_deadline {
        if Repository::open(&config)?
            .agent(&healthy_id)?
            .and_then(|record| record.instance)
            .is_some()
        {
            healthy_started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let backoffs: Vec<_> = {
        let coordination = manager.coordination();
        failing
            .iter()
            .map(|(id, _, _)| (id.clone(), coordination.restarts.contains_key(id)))
            .collect()
    };

    shutdown.cancel();
    manager.0.tasks.close();
    let _drained = tokio::time::timeout(Duration::from_secs(5), manager.0.tasks.wait()).await;
    for server in servers {
        let _joined = tokio::time::timeout(Duration::from_secs(2), server).await;
    }
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    // Then: `healthy` claimed a slot as soon as one freed up, and failing predecessors backed off.
    assert!(
        healthy_started,
        "healthy agent never claimed a start slot after failing supervisors dropped Terminate"
    );
    for (id, backed_off) in backoffs {
        assert!(
            backed_off,
            "failing predecessor recovery for {id} did not record retry backoff"
        );
    }
    Ok(())
}

/// Mock supervisor that drops the first `fail_count` Terminate requests, then ends the Process.
async fn run_flaky_terminate_supervisor(
    (address, credentials): (
        ma2a_services::supervisor::SupervisorAddress,
        ma2a_services::supervisor::Credentials,
    ),
    (fail_count, attempts, shutdown): (
        usize,
        Arc<std::sync::atomic::AtomicUsize>,
        CancellationToken,
    ),
) -> TestResult {
    use std::sync::atomic::Ordering;

    use interprocess::local_socket::tokio::prelude::*;
    use ma2a_core::service::{FrameKind, Termination};
    use ma2a_services::{
        FrameReader, FrameWriter,
        supervisor::{
            EndCause, SupervisorStatus,
            proof::{prove_supervisor, read_challenge},
        },
    };

    let listener = address.bind()?;
    loop {
        let mut stream = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => accepted?,
        };
        let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
        let (recv, send) = stream.split();
        let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
        prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
        let _request = reader.expect().await?;
        let seen = attempts.fetch_add(1, Ordering::SeqCst);
        if seen < fail_count {
            drop((reader, writer));
            continue;
        }
        let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 9_000);
        writer.frame(FrameKind::Outcome, &ended.to_bytes()).await?;
        return Ok(());
    }
}

struct MaintainEnv {
    dir: PathBuf,
    sockets_dir: PathBuf,
    config: StoreConfig,
    store: StoreClient,
    store_task: tokio::task::JoinHandle<()>,
    local: EndpointId,
    shutdown: CancellationToken,
    processes: ProcessManager,
    manager: AgentManager,
}

impl MaintainEnv {
    async fn new(label: &str) -> TestResult<Self> {
        let dir = short_state_dir(label)?;
        let sockets_dir = dir.join("run-v1").join("p");
        std::fs::create_dir_all(&sockets_dir)?;
        let config = StoreConfig::new(&dir);
        let backend = StoreBackend::open(&config)?;
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let shutdown = CancellationToken::new();
        let clock: Arc<dyn RuntimeClock> = Arc::new(TokioClock::new(1_000_000));
        let processes = ProcessManager::new(
            (local, store.clone(), Arc::clone(&clock)),
            (&dir, None),
            shutdown.clone(),
        );
        let manager = AgentManager::new(
            (local, store.clone(), clock),
            (&dir, processes.clone()),
            shutdown.clone(),
        );
        Ok(Self {
            dir,
            sockets_dir,
            config,
            store,
            store_task,
            local,
            shutdown,
            processes,
            manager,
        })
    }

    async fn tick_seconds(&self, seconds: usize) -> TestResult {
        for _ in 0..8 {
            let _rev = self.store.revision().await?;
            tokio::task::yield_now().await;
        }
        for _ in 0..seconds {
            for _ in 0..50 {
                tokio::time::advance(Duration::from_millis(20)).await;
                tokio::task::yield_now().await;
            }
            for _ in 0..8 {
                let _rev = self.store.revision().await?;
                tokio::task::yield_now().await;
            }
        }
        Ok(())
    }

    fn spawn_supervisor(
        &self,
        (pid, secret): (ProcessId, [u8; 32]),
        fail_count: usize,
    ) -> (
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<TestResult>,
    ) {
        use std::sync::atomic::AtomicUsize;

        use ma2a_services::supervisor::{Credentials, SupervisorAddress};

        let attempts = Arc::new(AtomicUsize::new(0));
        let address = SupervisorAddress::new(
            self.sockets_dir
                .join(format!("{pid}.sock"))
                .to_string_lossy()
                .into_owned(),
        );
        let handle = tokio::spawn(run_flaky_terminate_supervisor(
            (address, Credentials::new(pid, secret)),
            (fail_count, Arc::clone(&attempts), self.shutdown.clone()),
        ));
        (attempts, handle)
    }

    async fn finish(
        self,
        runner: tokio::task::JoinHandle<()>,
        supervisors: Vec<tokio::task::JoinHandle<TestResult>>,
    ) -> TestResult {
        self.shutdown.cancel();
        for _ in 0..20 {
            let _rev = self.store.revision().await?;
            tokio::task::yield_now().await;
            if runner.is_finished() && supervisors.iter().all(tokio::task::JoinHandle::is_finished)
            {
                break;
            }
        }
        if !runner.is_finished() {
            runner.abort();
        }
        let _joined = runner.await;
        for supervisor in supervisors {
            if !supervisor.is_finished() {
                supervisor.abort();
            }
            let _joined = supervisor.await;
        }
        drop(self.manager);
        drop(self.processes);
        self.store.stop().await?;
        self.store_task.await?;
        let _cleanup = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

fn seed_running_agent_process(
    (config, local): (&StoreConfig, EndpointId),
    (name, pid_byte, bind_process): (&str, u8, bool),
) -> TestResult<(AgentId, u64, ProcessId, [u8; 32])> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    let id = AgentId::new(name)?;
    let defined = repository
        .define_agent(
            &AgentDefinition {
                id: id.clone(),
                description: String::new(),
                start: start_bytes.clone(),
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?
        .into_value();
    let ma2a_store::AgentDefine::Defined(defined, _) = defined else {
        return Err("agent was not defined".into());
    };
    repository.set_agent_enabled(&id, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&id, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?
        .generation;
    let request_id = if bind_process {
        RequestId::random()?
    } else {
        super::instance::start_request(&id, (defined.times.0, generation))?
    };
    let pid = ProcessId::from_bytes([pid_byte; 16]);
    let secret = [pid_byte.wrapping_add(1); 32];
    repository.create_process(&NewProcess {
        id: pid,
        binding: (local, request_id),
        fingerprint: [pid_byte.wrapping_add(2); 32],
        start: (&start_bytes, None),
        secret,
        created_ms: 4,
    })?;
    for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
        repository.transition_process(
            pid,
            ProcessTransition {
                to,
                reason: None,
                termination: None,
                at_ms,
            },
        )?;
    }
    if bind_process {
        repository.bind_agent_process((&id, generation), pid)?;
    }
    Ok((id, generation, pid, secret))
}

#[tokio::test(start_paused = true)]
async fn failed_stop_termination_reports_error_and_maintenance_reconciles_disabled_agent()
-> TestResult {
    use std::sync::atomic::Ordering;

    use ma2a_core::service::ServiceError;

    // Given: an enabled Agent with a running Process whose supervisor drops the first Terminate
    // request and answers the second with Ended.
    let env = MaintainEnv::new("ma2a-stop-fail").await?;
    let (agent, generation, pid, secret) =
        seed_running_agent_process((&env.config, env.local), ("stop-retry", 0x51, true))?;
    let (attempts, supervisor) = env.spawn_supervisor((pid, secret), 1);
    tokio::task::yield_now().await;

    // When: operator stop commits disabled while the first Terminate fails, then maintenance runs.
    let stop_result = env.manager.stop(&agent).await;
    let after_stop_agent = Repository::open(&env.config)?
        .agent(&agent)?
        .ok_or("agent missing after stop")?;
    let after_stop_process = env
        .processes
        .local_record(pid)
        .await?
        .ok_or("process missing after stop")?;

    let runner = tokio::spawn(crate::agents::run(env.manager.clone()));
    env.tick_seconds(4).await?;

    let final_agent = Repository::open(&env.config)?
        .agent(&agent)?
        .ok_or("agent missing after maintenance")?;
    let final_process = env
        .processes
        .local_record(pid)
        .await?
        .ok_or("process missing after maintenance")?;
    env.finish(runner, vec![supervisor]).await?;

    // Then: stop reported Unavailable while committing disabled, and maintenance finished
    // terminating the Process without re-enabling or replacing the Agent.
    assert!(matches!(
        stop_result,
        Err((ServiceError::Unavailable, Some(_)))
    ));
    assert!(!after_stop_agent.enabled);
    assert_eq!(after_stop_process.state, ProcessState::Running);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(final_process.state, ProcessState::Terminated);
    assert!(!final_agent.enabled);
    assert_eq!(final_agent.generation, generation);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn restart_after_disable_commit_terminates_surviving_bound_and_unbound_processes()
-> TestResult {
    use std::sync::atomic::Ordering;

    // Given: two Agents disabled in SQLite just before daemon restart (one bound Process, one
    // unbound Process resolved via deterministic start RequestId), both still Running.
    let env = MaintainEnv::new("ma2a-stop-boot").await?;
    let (bound_agent, bound_gen, bound_pid, bound_secret) =
        seed_running_agent_process((&env.config, env.local), ("disabled-bound", 0x61, true))?;
    let (unbound_agent, unbound_gen, unbound_pid, unbound_secret) =
        seed_running_agent_process((&env.config, env.local), ("disabled-unbound", 0x64, false))?;
    {
        let mut repository = Repository::open(&env.config)?;
        repository.set_agent_enabled(&bound_agent, (false, 7))?;
        repository.set_agent_enabled(&unbound_agent, (false, 7))?;
    }
    let (bound_attempts, bound_sup) = env.spawn_supervisor((bound_pid, bound_secret), 0);
    let (unbound_attempts, unbound_sup) = env.spawn_supervisor((unbound_pid, unbound_secret), 0);
    tokio::task::yield_now().await;

    // When: the new Runtime starts Agent maintenance after restart.
    let runner = tokio::spawn(crate::agents::run(env.manager.clone()));
    env.tick_seconds(2).await?;

    let bound_record = Repository::open(&env.config)?
        .agent(&bound_agent)?
        .ok_or("bound agent missing")?;
    let unbound_record = Repository::open(&env.config)?
        .agent(&unbound_agent)?
        .ok_or("unbound agent missing")?;
    let bound_process = env
        .processes
        .local_record(bound_pid)
        .await?
        .ok_or("bound process missing")?;
    let unbound_process = env
        .processes
        .local_record(unbound_pid)
        .await?
        .ok_or("unbound process missing")?;
    env.finish(runner, vec![bound_sup, unbound_sup]).await?;

    // Then: maintenance terminated both surviving Processes while leaving both Agents disabled.
    assert_eq!(bound_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(unbound_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(bound_process.state, ProcessState::Terminated);
    assert_eq!(unbound_process.state, ProcessState::Terminated);
    assert!(!bound_record.enabled);
    assert!(!unbound_record.enabled);
    assert_eq!(bound_record.generation, bound_gen);
    assert_eq!(unbound_record.generation, unbound_gen);
    Ok(())
}

/// Handles one proven connection on the unready-attach mock supervisor.
async fn serve_unready_connection(
    mut stream: interprocess::local_socket::tokio::Stream,
    credentials: ma2a_services::supervisor::Credentials,
    (attach_count, term_count, fail_count, ended_tx, shutdown): (
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
        usize,
        Arc<tokio::sync::watch::Sender<bool>>,
        CancellationToken,
    ),
) -> TestResult {
    use std::sync::atomic::Ordering;

    use interprocess::local_socket::tokio::prelude::*;
    use ma2a_core::service::{FrameKind, Termination};
    use ma2a_services::{
        FrameReader, FrameWriter,
        supervisor::{
            EndCause, SupervisorRequest, SupervisorStatus,
            proof::{prove_supervisor, read_challenge},
            protocol::encode_reading,
        },
    };

    let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
    let (recv, send) = stream.split();
    let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
    prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
    let request = reader.expect().await?;
    let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 1_031_000);
    match SupervisorRequest::from_bytes(request.payload())? {
        SupervisorRequest::Attach(..) | SupervisorRequest::AttachStrict(..) => {
            attach_count.fetch_add(1, Ordering::SeqCst);
            let reading = encode_reading((0, 0, 0), false);
            writer.frame(FrameKind::Accept, &reading).await?;
            let mut ended_rx = ended_tx.subscribe();
            let done = tokio::select! {
                () = shutdown.cancelled() => false,
                res = ended_rx.wait_for(|done| *done) => res.is_ok(),
            };
            if done {
                let _sent = writer.frame(FrameKind::Outcome, &ended.to_bytes()).await;
            }
        }
        SupervisorRequest::Terminate(_) => {
            let seen = term_count.fetch_add(1, Ordering::SeqCst);
            if seen < fail_count {
                drop((reader, writer));
                return Ok(());
            }
            writer.frame(FrameKind::Outcome, &ended.to_bytes()).await?;
            let _sent = ended_tx.send(true);
        }
        _ => {}
    }
    Ok(())
}

/// Mock supervisor that keeps an Attach stream open without announcing `ready`, drops the first
/// `fail_count` Terminate connections, and terminates on the next one.
async fn run_unready_attach_supervisor(
    (address, credentials): (
        ma2a_services::supervisor::SupervisorAddress,
        ma2a_services::supervisor::Credentials,
    ),
    (attach_count, term_count, fail_count): (
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
        usize,
    ),
    shutdown: CancellationToken,
) -> TestResult {
    use interprocess::local_socket::tokio::prelude::*;

    let listener = address.bind()?;
    let ended_tx = Arc::new(tokio::sync::watch::channel(false).0);
    let mut handlers = Vec::new();
    loop {
        let stream = tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted?,
        };
        handlers.push(tokio::spawn(serve_unready_connection(
            stream,
            credentials,
            (
                Arc::clone(&attach_count),
                Arc::clone(&term_count),
                fail_count,
                Arc::clone(&ended_tx),
                shutdown.clone(),
            ),
        )));
    }
    for handler in handlers {
        handler.abort();
        let _joined = handler.await;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn overdue_unready_instance_retries_terminate_and_starts_replacement() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ma2a_services::supervisor::{Credentials, SupervisorAddress};

    // Given: an enabled Agent whose live generation-1 instance started at t=1_000_000, stays
    // attached without announcing `ready`, and whose supervisor drops the first Terminate request.
    let env = MaintainEnv::new("ma2a-ready-retry").await?;
    let (agent, generation, pid, secret) =
        seed_running_agent_process((&env.config, env.local), ("unready-retry", 0x71, true))?;
    rusqlite::Connection::open(env.config.database_path())?.execute(
        "UPDATE agent_instances SET started_at_ms = 1000000 WHERE agent_id = ?1",
        [agent.as_str()],
    )?;
    let attach_count = Arc::new(AtomicUsize::new(0));
    let term_count = Arc::new(AtomicUsize::new(0));
    let address = SupervisorAddress::new(
        env.sockets_dir
            .join(format!("{pid}.sock"))
            .to_string_lossy()
            .into_owned(),
    );
    let supervisor = tokio::spawn(run_unready_attach_supervisor(
        (address, Credentials::new(pid, secret)),
        (Arc::clone(&attach_count), Arc::clone(&term_count), 1),
        env.shutdown.clone(),
    ));
    tokio::task::yield_now().await;

    // When: maintenance attaches the link, stays attached before the 30 s ready deadline, then
    // crosses the deadline so the first Terminate fails and retries with backoff.
    let runner = tokio::spawn(crate::agents::run(env.manager.clone()));
    env.tick_seconds(1).await?;
    let attached_before_deadline = attach_count.load(Ordering::SeqCst);
    let terms_before_deadline = term_count.load(Ordering::SeqCst);

    tokio::time::advance(Duration::from_secs(28)).await;
    tokio::task::yield_now().await;
    env.tick_seconds(5).await?;

    let final_agent = Repository::open(&env.config)?
        .agent(&agent)?
        .ok_or("agent missing after overdue retry")?;
    let final_process = env
        .processes
        .local_record(pid)
        .await?
        .ok_or("process missing after overdue retry")?;
    env.finish(runner, vec![supervisor]).await?;

    // Then: the attachment remained live until the deadline, the failed Terminate was retried,
    // the old Process reached Terminated, and a replacement generation began.
    assert_eq!(attached_before_deadline, 1);
    assert_eq!(terms_before_deadline, 0);
    assert!(
        term_count.load(Ordering::SeqCst) >= 2,
        "overdue ready deadline did not retry Terminate"
    );
    assert_eq!(final_process.state, ProcessState::Terminated);
    assert!(
        final_agent.generation > generation,
        "replacement generation did not start after overdue instance ended: {final_agent:?}"
    );
    Ok(())
}

/// Handles one connection on [`run_rejecting_attach_supervisor`].
async fn serve_rejecting_attach_connection(
    mut stream: interprocess::local_socket::tokio::Stream,
    credentials: ma2a_services::supervisor::Credentials,
    (reject_error, attach_count, term_count, fail_term_count, terminated): (
        ma2a_core::service::ServiceError,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
        usize,
        Arc<std::sync::atomic::AtomicBool>,
    ),
) -> TestResult {
    use std::sync::atomic::Ordering;

    use interprocess::local_socket::tokio::prelude::*;
    use ma2a_core::service::{FrameKind, Termination};
    use ma2a_services::{
        FrameReader, FrameWriter,
        supervisor::{
            EndCause, SupervisorRequest, SupervisorStatus,
            proof::{prove_supervisor, read_challenge},
        },
    };

    let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 1_031_000);
    let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
    let (recv, send) = stream.split();
    let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
    prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
    let request = reader.expect().await?;
    match SupervisorRequest::from_bytes(request.payload())? {
        SupervisorRequest::Attach(..) | SupervisorRequest::AttachStrict(..) => {
            attach_count.fetch_add(1, Ordering::SeqCst);
            let error = if terminated.load(Ordering::SeqCst) {
                ma2a_core::service::ServiceError::NotRunning
            } else {
                reject_error
            };
            writer.reject(error).await?;
        }
        SupervisorRequest::Terminate(_) => {
            let seen = term_count.fetch_add(1, Ordering::SeqCst);
            if !terminated.load(Ordering::SeqCst) && seen < fail_term_count {
                drop((reader, writer));
                return Ok(());
            }
            terminated.store(true, Ordering::SeqCst);
            writer.frame(FrameKind::Outcome, &ended.to_bytes()).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Mock supervisor that rejects every `Attach` with `reject_error`, drops the first
/// `fail_term_count` `Terminate` requests, and ends the Process on the next one.
async fn run_rejecting_attach_supervisor(
    (address, credentials): (
        ma2a_services::supervisor::SupervisorAddress,
        ma2a_services::supervisor::Credentials,
    ),
    (reject_error, attach_count, term_count, fail_term_count): (
        ma2a_core::service::ServiceError,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
        usize,
    ),
    shutdown: CancellationToken,
) -> TestResult {
    use std::sync::atomic::AtomicBool;

    use interprocess::local_socket::tokio::prelude::*;

    let listener = address.bind()?;
    let terminated = Arc::new(AtomicBool::new(false));
    let mut handlers = Vec::new();
    loop {
        let stream = tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted?,
        };
        handlers.push(tokio::spawn(serve_rejecting_attach_connection(
            stream,
            credentials,
            (
                reject_error,
                Arc::clone(&attach_count),
                Arc::clone(&term_count),
                fail_term_count,
                Arc::clone(&terminated),
            ),
        )));
    }
    for handler in handlers {
        handler.abort();
        let _joined = handler.await;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn unready_instance_with_writer_busy_attach_is_terminated_at_ready_deadline() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ma2a_core::service::ServiceError;
    use ma2a_services::supervisor::{Credentials, SupervisorAddress};

    // Given: an enabled unready Agent whose supervisor always refuses Attach with WriterBusy
    // while accepting Terminate.
    let env = MaintainEnv::new("ma2a-busy-attach").await?;
    let (agent, generation, pid, secret) =
        seed_running_agent_process((&env.config, env.local), ("busy-unready", 0x73, true))?;
    rusqlite::Connection::open(env.config.database_path())?.execute(
        "UPDATE agent_instances SET started_at_ms = 1000000 WHERE agent_id = ?1",
        [agent.as_str()],
    )?;
    let attach_count = Arc::new(AtomicUsize::new(0));
    let term_count = Arc::new(AtomicUsize::new(0));
    let address = SupervisorAddress::new(
        env.sockets_dir
            .join(format!("{pid}.sock"))
            .to_string_lossy()
            .into_owned(),
    );
    let supervisor = tokio::spawn(run_rejecting_attach_supervisor(
        (address, Credentials::new(pid, secret)),
        (
            ServiceError::WriterBusy,
            Arc::clone(&attach_count),
            Arc::clone(&term_count),
            0,
        ),
        env.shutdown.clone(),
    ));
    tokio::task::yield_now().await;

    // When: maintenance runs past the 30 s ready deadline despite continuous Attach refusals.
    let runner = tokio::spawn(crate::agents::run(env.manager.clone()));
    let mut pre_deadline_seconds = 0_u64;
    while attach_count.load(Ordering::SeqCst) == 0 && pre_deadline_seconds < 10 {
        env.tick_seconds(1).await?;
        pre_deadline_seconds += 1;
    }
    let attached_before_deadline = attach_count.load(Ordering::SeqCst);
    let terms_before_deadline = term_count.load(Ordering::SeqCst);

    let remaining = 29_u64.saturating_sub(pre_deadline_seconds);
    if remaining > 0 {
        tokio::time::advance(Duration::from_secs(remaining)).await;
        tokio::task::yield_now().await;
    }
    for _ in 0..15 {
        env.tick_seconds(1).await?;
        let ended = env
            .processes
            .local_record(pid)
            .await?
            .is_some_and(|record| record.state == ProcessState::Terminated);
        let replaced = Repository::open(&env.config)?
            .agent(&agent)?
            .is_some_and(|record| record.generation > generation);
        if ended && replaced {
            break;
        }
    }

    let final_agent = Repository::open(&env.config)?
        .agent(&agent)?
        .ok_or("agent missing after WriterBusy ready deadline")?;
    let final_process = env
        .processes
        .local_record(pid)
        .await?
        .ok_or("process missing after WriterBusy ready deadline")?;
    env.finish(runner, vec![supervisor]).await?;

    // Then: the 30 s ready deadline fired anyway, terminated the Process, and started a
    // replacement generation.
    assert!(attached_before_deadline >= 1);
    assert_eq!(terms_before_deadline, 0);
    assert!(
        term_count.load(Ordering::SeqCst) >= 1,
        "ready deadline did not terminate instance whose Attach returned WriterBusy"
    );
    assert_eq!(final_process.state, ProcessState::Terminated);
    assert!(
        final_agent.generation > generation,
        "replacement generation did not start after WriterBusy instance ended: {final_agent:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stdin_closed_running_instance_retries_terminate_and_starts_replacement() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ma2a_core::service::ServiceError;
    use ma2a_services::supervisor::{Credentials, SupervisorAddress};
    use ma2a_store::{AgentOutput, AgentOutputEffect};

    // Given: an enabled ready Agent whose running Process rejects Attach with StdinClosed and
    // whose supervisor drops the first Terminate before answering the second with Ended.
    let env = MaintainEnv::new("ma2a-stdin-closed").await?;
    let (agent, generation, pid, secret) =
        seed_running_agent_process((&env.config, env.local), ("stdin-closed", 0x75, true))?;
    Repository::open(&env.config)?.apply_agent_output(
        &AgentOutput {
            agent: agent.clone(),
            generation,
            cursor: 1,
            at_ms: 1_000_000,
        },
        AgentOutputEffect::Ready,
    )?;
    let attach_count = Arc::new(AtomicUsize::new(0));
    let term_count = Arc::new(AtomicUsize::new(0));
    let address = SupervisorAddress::new(
        env.sockets_dir
            .join(format!("{pid}.sock"))
            .to_string_lossy()
            .into_owned(),
    );
    let supervisor = tokio::spawn(run_rejecting_attach_supervisor(
        (address, Credentials::new(pid, secret)),
        (
            ServiceError::StdinClosed,
            Arc::clone(&attach_count),
            Arc::clone(&term_count),
            1,
        ),
        env.shutdown.clone(),
    ));
    tokio::task::yield_now().await;

    // When: maintenance runs for up to 10 s (well under the 30 s ready deadline).
    let runner = tokio::spawn(crate::agents::run(env.manager.clone()));
    for _ in 0..10 {
        env.tick_seconds(1).await?;
        let ended = env
            .processes
            .local_record(pid)
            .await?
            .is_some_and(|record| record.state == ProcessState::Terminated);
        let replaced = Repository::open(&env.config)?
            .agent(&agent)?
            .is_some_and(|record| record.generation > generation);
        if ended && replaced {
            break;
        }
    }

    let final_agent = Repository::open(&env.config)?
        .agent(&agent)?
        .ok_or("agent missing after StdinClosed termination")?;
    let final_process = env
        .processes
        .local_record(pid)
        .await?
        .ok_or("process missing after StdinClosed termination")?;
    env.finish(runner, vec![supervisor]).await?;

    // Then: the link terminated the Process (retrying the dropped Terminate), concluded the
    // generation, and started a replacement instead of looping forever as Broken.
    assert!(attach_count.load(Ordering::SeqCst) >= 1);
    assert!(
        term_count.load(Ordering::SeqCst) >= 2,
        "StdinClosed did not retry Terminate"
    );
    assert_eq!(final_process.state, ProcessState::Terminated);
    assert!(
        final_agent.generation > generation,
        "replacement generation did not start after StdinClosed instance ended: {final_agent:?}"
    );
    Ok(())
}

async fn wait_for_ready_instance(
    config: &StoreConfig,
    agent: &AgentId,
) -> Result<(u64, ProcessId), Box<dyn std::error::Error + Send + Sync>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(record) = Repository::open(config)?.agent(agent)?
            && let Some(instance) = record.instance
            && instance.ready
            && let Some(pid) = instance.process_id
        {
            return Ok((instance.generation, pid));
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "generation 1 did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_agent_closing_stdin_while_attached_is_terminated_and_replaced() -> TestResult {
    use ma2a_core::service::AgentSubmit;

    // Given: a real supervised Agent whose first generation announces ready, waits for the
    // trigger file, closes its stdin FD while staying alive, and whose replacement stays open.
    let env = MaintainEnv::new("ma2a-live-stdin-closed").await?;
    let hosted_processes = ProcessManager::new(
        (
            env.local,
            env.store.clone(),
            Arc::new(crate::clock::SystemClock),
        ),
        (&env.dir, Some(ProcessHosting::in_process())),
        env.shutdown.clone(),
    );
    let manager = AgentManager::new(
        (
            env.local,
            env.store.clone(),
            Arc::new(crate::clock::SystemClock),
        ),
        (&env.dir, hosted_processes.clone()),
        env.shutdown.clone(),
    );
    let script = concat!(
        "marker=\"$MA2A_AGENT_WORKSPACE/once\"; ",
        "if [ ! -f \"$marker\" ]; then ",
        ": > \"$marker\"; ",
        "printf '{\"t\":\"ready\",\"protocol\":1}\\n'; ",
        "while [ ! -f \"$MA2A_AGENT_WORKSPACE/close_stdin\" ]; do sleep 0.05; done; ",
        "exec <&-; ",
        ": > \"$MA2A_AGENT_WORKSPACE/stdin_closed\"; ",
        "exec sleep 60; ",
        "else ",
        "printf '{\"t\":\"ready\",\"protocol\":1}\\n'; ",
        "exec sleep 60; ",
        "fi"
    );
    let agent = AgentId::new("stdin-closer")?;
    let spec = crate::agents::operator::AgentSpec {
        id: agent.clone(),
        description: String::new(),
        command: (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), script.to_owned()],
        ),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    };
    manager.define(spec).await?;
    manager.start(&agent).await?;
    let workspace = manager.workspace(&agent);

    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    let gen1 = wait_for_ready_instance(&env.config, &agent).await?;

    // When: the already-attached child closes stdin while staying alive, and a task is delivered.
    std::fs::write(workspace.join("close_stdin"), b"1")?;
    let closed_marker = workspace.join("stdin_closed");
    let close_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !closed_marker.exists() {
        assert!(
            tokio::time::Instant::now() < close_deadline,
            "child did not close stdin"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let submit = AgentSubmit::new(
        agent.clone(),
        AgentMessage::text(MessageRole::User, "hello")?,
        None,
    );
    manager
        .submit((env.local, RequestId::random()?), &submit)
        .await?;

    let replace_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut replaced = false;
    while tokio::time::Instant::now() < replace_deadline {
        let ended = hosted_processes
            .local_record(gen1.1)
            .await?
            .is_some_and(|record| record.state == ProcessState::Terminated);
        let next_ready = Repository::open(&env.config)?
            .agent(&agent)?
            .and_then(|record| record.instance)
            .is_some_and(|instance| instance.generation > gen1.0 && instance.ready);
        if ended && next_ready {
            replaced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let gen1_process = hosted_processes.local_record(gen1.1).await?;
    let final_agent = Repository::open(&env.config)?.agent(&agent)?;
    let _stopped = manager.stop(&agent).await;
    drop(manager);
    drop(hosted_processes);
    env.finish(runner, Vec::new()).await?;

    // Then: generation 1 was terminated and a replacement generation became ready within 20 s.
    assert!(
        replaced,
        "child closing stdin while attached was not terminated/replaced: gen1={gen1_process:?}, agent={final_agent:?}"
    );
    Ok(())
}
