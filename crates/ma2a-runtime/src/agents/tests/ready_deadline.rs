//! `ready_deadline` stays bounded by `READY_DEADLINE_MS` across wall-clock rollbacks and clock errors.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_core::{
    EndpointId, RequestId,
    service::{AgentId, MAX_LOG_LIMIT, ProcessId, ProcessSpec, ProcessStart, ProcessState},
};
use ma2a_store::{AgentDefinition, NewProcess, ProcessTransition, Repository, StoreConfig};
use tokio::sync::{Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    RuntimeClock, RuntimeError,
    error::RuntimeErrorKind,
    processes::ProcessManager,
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct StepClock {
    value: Mutex<Result<i64, ()>>,
    calls: AtomicUsize,
    called: Notify,
}

impl StepClock {
    fn new(now_ms: i64) -> Self {
        Self {
            value: Mutex::new(Ok(now_ms)),
            calls: AtomicUsize::new(0),
            called: Notify::new(),
        }
    }

    fn set(&self, next: Result<i64, ()>) {
        if let Ok(mut guard) = self.value.lock() {
            *guard = next;
        }
    }

    async fn wait_for_call(&self) {
        while self.calls.load(Ordering::SeqCst) == 0 {
            let notified = self.called.notified();
            if self.calls.load(Ordering::SeqCst) > 0 {
                break;
            }
            notified.await;
        }
    }
}

impl RuntimeClock for StepClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        let state = *self
            .value
            .lock()
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.called.notify_waiters();
        state.map_err(|()| RuntimeError::new(RuntimeErrorKind::Clock))
    }
}

struct ReadyEnv {
    dir: PathBuf,
    store: StoreClient,
    store_task: tokio::task::JoinHandle<()>,
    shutdown: CancellationToken,
    clock: Arc<StepClock>,
    processes: ProcessManager,
    manager: AgentManager,
    agent: AgentId,
    generation: u64,
    pid: ProcessId,
}

impl ReadyEnv {
    async fn new(label: &str, started_ms: i64) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{label}-{:x}-{serial}", std::process::id()));
        super::private_dir(&dir)?;
        let config = StoreConfig::new(&dir);
        let backend = StoreBackend::open(&config)?;
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let (agent, generation, pid) = seed_unready(&config, local, started_ms)?;

        let shutdown = CancellationToken::new();
        let clock = Arc::new(StepClock::new(started_ms));
        let clock_dyn: Arc<dyn RuntimeClock> = Arc::clone(&clock) as Arc<dyn RuntimeClock>;
        let processes = ProcessManager::new(
            (local, store.clone(), Arc::clone(&clock_dyn)),
            (&dir, None),
            shutdown.clone(),
        );
        let manager = AgentManager::new(
            (local, store.clone(), clock_dyn),
            (&dir, processes.clone()),
            shutdown.clone(),
        );
        Ok(Self {
            dir,
            store,
            store_task,
            shutdown,
            clock,
            processes,
            manager,
            agent,
            generation,
            pid,
        })
    }

    async fn finish(self, task: tokio::task::JoinHandle<()>) -> TestResult {
        self.shutdown.cancel();
        task.abort();
        let _joined = task.await;
        drop(self.manager);
        drop(self.processes);
        self.store.stop().await?;
        self.store_task.await?;
        let _cleanup = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

fn seed_unready(
    config: &StoreConfig,
    local: EndpointId,
    started_ms: i64,
) -> TestResult<(AgentId, u64, ProcessId)> {
    let started_u64 = u64::try_from(started_ms)?;
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    let agent = AgentId::new("ready-clock-test")?;
    repository.define_agent(
        &AgentDefinition {
            id: agent.clone(),
            description: String::new(),
            start: start_bytes.clone(),
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&agent, started_u64)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("missing instance")?
        .generation;
    let pid = ProcessId::from_bytes([0x42; 16]);
    repository.create_process(&NewProcess {
        id: pid,
        binding: (local, RequestId::random()?),
        fingerprint: [0x43; 32],
        start: (&start_bytes, None),
        secret: [0x44; 32],
        created_ms: started_u64,
    })?;
    for (to, at_ms) in [
        (ProcessState::Starting, started_u64),
        (ProcessState::Running, started_u64),
    ] {
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
    repository.bind_agent_process((&agent, generation), pid)?;
    Ok((agent, generation, pid))
}

async fn assert_terminates_at_deadline(label: &str, next_clock: Result<i64, ()>) -> TestResult {
    let env = ReadyEnv::new(label, 1_700_000_000_000).await?;
    env.clock.set(next_clock);
    let (terminated_tx, mut terminated_rx) = watch::channel(false);
    let manager = env.manager.clone();
    let agent = env.agent.clone();
    let generation = env.generation;
    let task = tokio::spawn(async move {
        manager
            .ready_deadline((&agent, generation), &terminated_tx)
            .await;
    });
    env.clock.wait_for_call().await;
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(29)).await;
    let _rev = env.store.revision().await?;
    tokio::task::yield_now().await;
    assert!(
        !*terminated_rx.borrow(),
        "ready_deadline terminated before 30 s elapsed"
    );

    tokio::time::advance(Duration::from_secs(2)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !*terminated_rx.borrow_and_update() && std::time::Instant::now() < deadline {
        let _rev = env.store.revision().await?;
        tokio::task::yield_now().await;
        if !*terminated_rx.borrow() {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let terminated = *terminated_rx.borrow_and_update();
    let process = env
        .processes
        .local_record(env.pid)
        .await?
        .ok_or("process missing")?;
    env.finish(task).await?;

    assert!(
        terminated,
        "unready instance was not terminated at ready deadline"
    );
    assert!(
        process.state.is_terminal(),
        "process did not reach terminal state: {:?}",
        process.state
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn ready_deadline_stays_bounded_after_wall_clock_rollback() -> TestResult {
    assert_terminates_at_deadline("ma2a-rd-back", Ok(1_700_000_000_000 - 3_600_000)).await
}

#[tokio::test(start_paused = true)]
async fn ready_deadline_stays_bounded_when_clock_errors() -> TestResult {
    assert_terminates_at_deadline("ma2a-rd-err", Err(())).await
}
