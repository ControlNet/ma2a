//! Hosted Processes: durable workloads owned by per-Process supervisors.
//!
//! `SQLite` is the truth for every Process. A supervisor is adopted only through the mutual
//! ownership proof, never by a process identifier. Every non-terminal Process has at most
//! one follower task in this Runtime, which turns its supervisor's reports into guarded
//! durable transitions; when no supervisor can be proven, only its status record decides.

mod apply;
mod attach;
mod follow;
mod host;
mod launch;
mod local;
mod maintain;
mod reads;
mod start;
mod stdin;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use ma2a_core::{
    EndpointId,
    service::{ProcessId, ProcessReason, ServiceError},
};
use ma2a_services::supervisor::{Credentials, SupervisorAddress};
use ma2a_store::ProcessRecord;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) use host::serve;
pub use launch::ProcessHosting;
pub(crate) use maintain::run;

use crate::{RuntimeClock, store::StoreClient};

/// Name of the directory holding one subdirectory per Process.
const PROCESSES_DIR: &str = "processes-v1";

/// In-memory coordination; everything that must survive a restart is in the Store.
#[derive(Debug, Default)]
struct Coordination {
    /// Processes with a running follower.
    followed: HashSet<ProcessId>,
    /// Why this Runtime asked a Process to terminate, until that end is recorded.
    terminating: HashMap<ProcessId, ProcessReason>,
    /// Attached readers per Process.
    readers: HashMap<ProcessId, usize>,
}

#[derive(Debug)]
struct Inner {
    local: EndpointId,
    store: StoreClient,
    clock: Arc<dyn RuntimeClock>,
    hosting: Option<ProcessHosting>,
    processes_dir: PathBuf,
    sockets_dir: PathBuf,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    coordination: Mutex<Coordination>,
}

/// Every Process this Runtime hosts; cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct ProcessManager(Arc<Inner>);

impl ProcessManager {
    /// Creates the manager for one state directory; without `hosting`, `start` is refused
    /// but existing Processes are still followed, read, and managed.
    pub(crate) fn new(
        (local, store, clock): (EndpointId, StoreClient, Arc<dyn RuntimeClock>),
        (state_dir, hosting): (&Path, Option<ProcessHosting>),
        shutdown: CancellationToken,
    ) -> Self {
        Self(Arc::new(Inner {
            local,
            store,
            clock,
            hosting,
            processes_dir: state_dir.join(PROCESSES_DIR),
            sockets_dir: state_dir.join("run-v1").join("p"),
            shutdown,
            tasks: TaskTracker::new(),
            coordination: Mutex::new(Coordination::default()),
        }))
    }

    fn coordination(&self) -> MutexGuard<'_, Coordination> {
        self.0
            .coordination
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self) -> &StoreClient {
        &self.0.store
    }

    fn now_ms(&self) -> u64 {
        self.0
            .clock
            .now_ms()
            .ok()
            .and_then(|now| u64::try_from(now).ok())
            .unwrap_or(0)
    }

    /// Returns the directory of one Process: its status record and output.
    fn directory(&self, id: ProcessId) -> PathBuf {
        self.0.processes_dir.join(id.to_string())
    }

    /// Returns where the supervisor of one Process listens.
    fn address(&self, id: ProcessId) -> SupervisorAddress {
        #[cfg(unix)]
        let address = self
            .0
            .sockets_dir
            .join(format!("{id}.sock"))
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let address = {
            let scope = blake3::hash(self.0.processes_dir.as_os_str().as_encoded_bytes());
            let scope = scope.to_hex();
            format!("ma2a-p-{}-{id}", scope.get(..16).unwrap_or_default())
        };
        SupervisorAddress::new(address)
    }

    /// Returns the Store revision, which every Process commit advances.
    pub(crate) async fn revision(&self) -> Result<u64, ServiceError> {
        self.store()
            .processes(|repository| repository.revision())
            .await?
            .map_err(|_| ServiceError::Unavailable)
    }

    /// Returns one Process record.
    async fn record(&self, id: ProcessId) -> Result<Option<ProcessRecord>, ServiceError> {
        self.store()
            .processes(move |repository| repository.process(id))
            .await?
            .map_err(|_| ServiceError::Unavailable)
    }
}

/// The ownership credentials of a non-terminal Process.
fn credentials(record: &ProcessRecord) -> Option<Credentials> {
    record
        .secret
        .map(|secret| Credentials::new(record.id, secret))
}

/// Returns the record when `peer` created it; any other Process does not exist for `peer`.
fn owned(record: Option<ProcessRecord>, peer: EndpointId) -> Result<ProcessRecord, ServiceError> {
    record
        .filter(|record| record.creator == peer)
        .ok_or(ServiceError::UnknownProcess)
}

#[cfg(test)]
mod tests;
