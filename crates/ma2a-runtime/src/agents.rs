//! Hosted Agents: logical workloads whose instances are local Processes.
//!
//! `SQLite` is the truth for every Agent, instance, task, and delivery. An instance is a
//! Process the local Endpoint created for itself; one link per live instance follows its
//! output from a durable cursor, applies every adapter line exactly once, and writes
//! unacknowledged deliveries to its stdin. Tools run as same-user stream-plane clients of
//! this Runtime, so an Agent never holds more authority than its hosting Endpoint.

mod artifact_read;
mod artifacts;
mod convert;
mod host;
mod instance;
mod lines;
mod link;
mod maintain;
mod operator;
mod render;
mod tasks;
mod tools;
mod watch;

#[cfg(test)]
#[path = "agents/tests/link.rs"]
mod link_tests;
#[cfg(test)]
#[path = "agents/tests/live.rs"]
mod live_tests;
#[cfg(test)]
#[path = "agents/tests/maintain.rs"]
mod maintain_tests;
#[cfg(test)]
#[path = "agents/tests/ready_deadline.rs"]
mod ready_deadline_tests;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak},
    time::Duration,
};

use ma2a_core::{
    EndpointId,
    service::{AgentId, AgentTaskId, ServiceError},
};
use ma2a_store::{AgentRecord, AgentTaskRecord, Repository};
use tokio::sync::watch as signal;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) use host::serve;
pub(crate) use maintain::run;
pub(crate) use operator::AgentSpec;
pub(crate) use tasks::Sent;

use crate::{RuntimeClock, ipc::IpcPaths, processes::ProcessManager, store::StoreClient};

/// Name of the directory holding every Agent's workspace and the artifact blobs.
const AGENTS_DIR: &str = "agents-v1";
/// Most Agent instance startups or recoveries running concurrently in maintenance.
const MAX_CONCURRENT_STARTS: usize = 8;

/// In-memory coordination; everything that must survive a restart is in the Store.
#[derive(Debug, Default)]
struct Coordination {
    /// The instance generation each Agent's running link follows.
    links: HashMap<AgentId, u64>,
    /// Agents whose instance startup or recovery task is currently running.
    starting: HashSet<AgentId>,
    /// When each Agent may start its next instance, and the backoff after that.
    restarts: HashMap<AgentId, (tokio::time::Instant, Duration)>,
    /// Tool calls being executed now.
    calls: HashSet<(AgentTaskId, String)>,
    /// Listeners that live as long as their task.
    listeners: HashMap<AgentTaskId, Vec<CancellationToken>>,
    /// Per-Agent lock serializing lifecycle changes through process termination.
    lifecycle: HashMap<AgentId, Weak<tokio::sync::Mutex<()>>>,
}

#[derive(Debug)]
struct Inner {
    local: EndpointId,
    store: StoreClient,
    clock: Arc<dyn RuntimeClock>,
    processes: ProcessManager,
    dir: PathBuf,
    loopback: OnceLock<IpcPaths>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    /// Bounds concurrent background Agent starts in maintenance.
    start_permits: Arc<tokio::sync::Semaphore>,
    /// Advanced after every change of an Agent, instance, task, or delivery.
    changes: signal::Sender<u64>,
    coordination: Mutex<Coordination>,
    /// Serializes placing and referencing a blob against collecting unreferenced ones.
    blob_lock: tokio::sync::Mutex<()>,
}

/// Every Agent this Runtime hosts; cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct AgentManager(Arc<Inner>);

impl AgentManager {
    /// Creates the manager for one state directory.
    pub(crate) fn new(
        (local, store, clock): (EndpointId, StoreClient, Arc<dyn RuntimeClock>),
        (state_dir, processes): (&Path, ProcessManager),
        shutdown: CancellationToken,
    ) -> Self {
        Self(Arc::new(Inner {
            local,
            store,
            clock,
            processes,
            dir: state_dir.join(AGENTS_DIR),
            loopback: OnceLock::new(),
            shutdown,
            tasks: TaskTracker::new(),
            start_permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_STARTS)),
            changes: signal::Sender::new(0),
            coordination: Mutex::new(Coordination::default()),
            blob_lock: tokio::sync::Mutex::new(()),
        }))
    }

    /// Lets tools reach other Endpoints through this Runtime's own local API.
    pub(crate) fn install_loopback(&self, paths: IpcPaths) {
        let _installed = self.0.loopback.set(paths);
    }

    /// Starts this manager's shutdown ahead of the Runtime's.
    ///
    /// The local API calls this before it closes its connections: a tool call relayed through
    /// one of them must see the shutdown first, or it records the closed connection as its
    /// answer instead of staying pending for the next start.
    pub(crate) fn begin_shutdown(&self) {
        self.0.shutdown.cancel();
    }

    fn coordination(&self) -> MutexGuard<'_, Coordination> {
        self.0
            .coordination
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Serializes lifecycle operations and instance launches for one Agent.
    async fn lock_lifecycle(&self, agent: &AgentId) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut coordination = self.coordination();
            coordination
                .lifecycle
                .retain(|_, entry| entry.strong_count() > 0);
            coordination
                .lifecycle
                .get(agent)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    coordination
                        .lifecycle
                        .insert(agent.clone(), Arc::downgrade(&lock));
                    lock
                })
        };
        lock.lock_owned().await
    }

    fn now_ms(&self) -> u64 {
        self.0
            .clock
            .now_ms()
            .ok()
            .and_then(|now| u64::try_from(now).ok())
            .unwrap_or(0)
    }

    /// Wakes every link, watcher, and waiter to re-read durable state.
    fn changed(&self) {
        self.0
            .changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// Returns a receiver woken by every later change.
    fn subscribe(&self) -> signal::Receiver<u64> {
        self.0.changes.subscribe()
    }

    /// Runs one repository job on the store thread, mapping every failure to unavailable.
    async fn store<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> Result<T, ma2a_store::StoreError> + Send + 'static,
    ) -> Result<T, ServiceError> {
        self.0
            .store
            .processes(job)
            .await?
            .map_err(|_| ServiceError::Unavailable)
    }

    /// Runs one committing job and wakes everyone when it succeeded.
    async fn commit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> Result<ma2a_store::Committed<T>, ma2a_store::StoreError>
        + Send
        + 'static,
    ) -> Result<T, ServiceError> {
        Ok(self.commit_at(job).await?.0)
    }

    /// Runs one committing job, as [`Self::commit`], and also returns its commit's revision.
    async fn commit_at<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> Result<ma2a_store::Committed<T>, ma2a_store::StoreError>
        + Send
        + 'static,
    ) -> Result<(T, u64), ServiceError> {
        let committed = self.store(job).await?;
        let revision = committed.revision();
        self.changed();
        Ok((committed.into_value(), revision))
    }

    /// Returns the Endpoint this Runtime is: the creator of every local task.
    pub(crate) fn local(&self) -> ma2a_core::EndpointId {
        self.0.local
    }

    async fn agent(&self, id: &AgentId) -> Result<Option<AgentRecord>, ServiceError> {
        let id = id.clone();
        self.store(move |repository| repository.agent(&id)).await
    }

    async fn task(&self, id: AgentTaskId) -> Result<Option<AgentTaskRecord>, ServiceError> {
        self.store(move |repository| repository.agent_task(id))
            .await
    }

    /// Returns the directory of one Agent: its workspace lives below it.
    fn workspace(&self, agent: &AgentId) -> PathBuf {
        self.0.dir.join(agent.as_str()).join("workspace")
    }

    /// Returns the content-addressed artifact store.
    fn blobs(&self) -> PathBuf {
        self.0.dir.join("blobs")
    }
}

/// Creates one directory readable only by this user.
fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Returns the record when `viewer` created it; the local operator (`None`) sees every task.
fn visible(
    record: Option<AgentTaskRecord>,
    (viewer, agent): (Option<EndpointId>, Option<&AgentId>),
) -> Result<AgentTaskRecord, ServiceError> {
    record
        .filter(|record| viewer.is_none_or(|peer| record.summary.creator == peer))
        .filter(|record| agent.is_none_or(|agent| record.summary.agent == *agent))
        .ok_or(ServiceError::UnknownTask)
}

/// Aborts this daemon at one named point (debug builds only).
fn fault(point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os("MA2A_TEST_AGENT_FAULT").is_some_and(|value| value == point) {
        ma2a_services::supervisor::crash();
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}
