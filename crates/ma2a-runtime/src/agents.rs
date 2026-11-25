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
#[path = "agents/tests/live.rs"]
mod live_tests;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
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

use crate::{RuntimeClock, ipc::IpcPaths, processes::ProcessManager, store::StoreClient};

/// Name of the directory holding every Agent's workspace and the artifact blobs.
const AGENTS_DIR: &str = "agents-v1";

/// In-memory coordination; everything that must survive a restart is in the Store.
#[derive(Debug, Default)]
struct Coordination {
    /// The instance generation each Agent's running link follows.
    links: HashMap<AgentId, u64>,
    /// When each Agent may start its next instance, and the backoff after that.
    restarts: HashMap<AgentId, (tokio::time::Instant, Duration)>,
    /// Tool calls being executed now.
    calls: HashSet<(AgentTaskId, String)>,
    /// Listeners that live as long as their task.
    listeners: HashMap<AgentTaskId, Vec<CancellationToken>>,
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
            changes: signal::Sender::new(0),
            coordination: Mutex::new(Coordination::default()),
            blob_lock: tokio::sync::Mutex::new(()),
        }))
    }

    /// Lets tools reach other Endpoints through this Runtime's own local API.
    pub(crate) fn install_loopback(&self, paths: IpcPaths) {
        let _installed = self.0.loopback.set(paths);
    }

    fn coordination(&self) -> MutexGuard<'_, Coordination> {
        self.0
            .coordination
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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

    /// Returns the Store revision, which every committed operator change has reached.
    pub(crate) async fn revision(&self) -> Result<u64, ServiceError> {
        self.store(|repository| repository.revision()).await
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
