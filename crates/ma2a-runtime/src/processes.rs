//! Hosted Processes: durable workloads owned by per-Process supervisors.
//!
//! `SQLite` is the truth for every Process. A supervisor is adopted only through the mutual
//! ownership proof, never by a process identifier. Every non-terminal Process has at most
//! one follower task in this Runtime, which turns its supervisor's reports into guarded
//! durable transitions; when no supervisor can be proven, only its status record decides.

mod apply;
mod attach;
mod follow;
mod go;
mod host;
mod launch;
mod local;
mod maintain;
mod reads;
mod relay;
mod start;
mod stdin;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use ma2a_core::{
    EndpointId,
    service::{ProcessId, ProcessReason, ServiceAccess, ServiceError},
};
use ma2a_services::supervisor::{Credentials, SupervisorAddress};
use ma2a_store::{Committed, ProcessRecord};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) use host::serve;
pub use launch::ProcessHosting;
pub(crate) use maintain::run;

use crate::{RuntimeClock, store::StoreClient};

/// Name of the directory holding one subdirectory per Process.
const PROCESSES_DIR: &str = "processes-v1";

/// In-flight termination state for one Process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Terminating {
    reason: ProcessReason,
    in_flight: usize,
    unauthorized_in_flight: usize,
    keep: bool,
}

impl Terminating {
    #[cfg(all(test, unix))]
    const fn new(reason: ProcessReason) -> Self {
        Self {
            reason,
            in_flight: 0,
            unauthorized_in_flight: 0,
            keep: false,
        }
    }
}

/// In-memory coordination; everything that must survive a restart is in the Store.
#[derive(Debug, Default)]
struct Coordination {
    /// Processes with a running follower.
    followed: HashSet<ProcessId>,
    /// `created` Processes with an active bounded-backoff settlement task.
    settling: HashSet<ProcessId>,
    /// Why this Runtime asked a Process to terminate, until that end is recorded.
    terminating: HashMap<ProcessId, Terminating>,
    /// `starting` Processes whose `go` dispatch was aborted by withdrawal or hold expiry.
    aborted_starting: HashMap<ProcessId, go::AbortCause>,
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
    /// What names this state directory's supervisor pipes, whichever spelling opened it.
    #[cfg(windows)]
    pipe_scope: String,
    shutdown: CancellationToken,
    /// Open once startup recovery has scanned prior-boot rows; new starts wait while closed.
    admit: tokio::sync::watch::Sender<bool>,
    tasks: TaskTracker,
    coordination: Mutex<Coordination>,
    #[cfg(test)]
    gates: go::GoTestGates,
}

/// Every Process this Runtime hosts; cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct ProcessManager(Arc<Inner>);

/// Tracks one in-flight `terminate` call so a failing concurrent call does not drop the
/// reason of a call still waiting, while clearing an unended Process once all calls finish.
pub(super) struct TerminateGuard<'a> {
    manager: &'a ProcessManager,
    id: ProcessId,
    reason: ProcessReason,
}

impl Drop for TerminateGuard<'_> {
    fn drop(&mut self) {
        let mut coordination = self.manager.coordination();
        let Some(entry) = coordination.terminating.get_mut(&self.id) else {
            return;
        };
        entry.in_flight = entry.in_flight.saturating_sub(1);
        if self.reason == ProcessReason::Unauthorized {
            entry.unauthorized_in_flight = entry.unauthorized_in_flight.saturating_sub(1);
        }
        if entry.in_flight == 0 && !entry.keep {
            coordination.terminating.remove(&self.id);
        }
    }
}

impl ProcessManager {
    pub(super) fn enter_terminating(
        &self,
        id: ProcessId,
        reason: ProcessReason,
    ) -> TerminateGuard<'_> {
        let unauthorized = usize::from(reason == ProcessReason::Unauthorized);
        let mut coordination = self.coordination();
        match coordination.terminating.get_mut(&id) {
            Some(entry) => {
                entry.in_flight += 1;
                entry.unauthorized_in_flight += unauthorized;
                if reason == ProcessReason::Unauthorized {
                    entry.reason = ProcessReason::Unauthorized;
                }
            }
            None => {
                coordination.terminating.insert(
                    id,
                    Terminating {
                        reason,
                        in_flight: 1,
                        unauthorized_in_flight: unauthorized,
                        keep: false,
                    },
                );
            }
        }
        drop(coordination);
        TerminateGuard {
            manager: self,
            id,
            reason,
        }
    }

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
            #[cfg(windows)]
            pipe_scope: crate::ipc::directory_identity(state_dir)
                .to_hex()
                .get(..16)
                .unwrap_or_default()
                .to_owned(),
            shutdown,
            admit: tokio::sync::watch::Sender::new(true),
            tasks: TaskTracker::new(),
            coordination: Mutex::new(Coordination::default()),
            #[cfg(test)]
            gates: go::GoTestGates::default(),
        }))
    }

    /// Holds new Process admission until [`Self::reconcile`] has scanned prior-boot rows.
    pub(crate) fn fence_startup(&self) {
        self.0.admit.send_replace(false);
    }

    /// Releases the startup admission fence once prior-boot rows have been scanned.
    pub(super) fn release_startup(&self) {
        self.0.admit.send_replace(true);
    }

    /// Waits until the startup recovery scan has claimed prior-boot rows, or `stop` cancels.
    pub(super) async fn wait_for_startup(
        &self,
        stop: Option<&CancellationToken>,
    ) -> Result<(), ServiceError> {
        if stop.is_some_and(CancellationToken::is_cancelled) {
            return Err(ServiceError::Unavailable);
        }
        let mut admitted = self.0.admit.subscribe();
        while !*admitted.borrow_and_update() {
            tokio::select! {
                biased;
                () = self.0.shutdown.cancelled() => return Err(ServiceError::Unavailable),
                () = async {
                    match stop {
                        Some(stop) => stop.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => return Err(ServiceError::Unavailable),
                updated = admitted.changed() => {
                    if updated.is_err() {
                        return Err(ServiceError::Unavailable);
                    }
                }
            }
        }
        Ok(())
    }

    /// Rechecks durable `Process` authorization for a remote creator; local Processes need none.
    pub(super) async fn authorize_remote(&self, creator: EndpointId) -> Result<(), ServiceError> {
        if creator == self.0.local {
            return Ok(());
        }
        self.store()
            .authorize_service((self.0.local, creator), ServiceAccess::Process)
            .await
            .map(|_| ())
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
        let address = format!("ma2a-p-{}-{id}", self.0.pipe_scope);
        SupervisorAddress::new(address)
    }

    /// Returns one Process record.
    async fn record(&self, id: ProcessId) -> Result<Option<ProcessRecord>, ServiceError> {
        Ok(self.record_at(id).await?.into_value())
    }

    /// Returns one Process record with the Store revision it was read at.
    ///
    /// Both are read in one transaction, so they come from the same snapshot even when another
    /// connection to the database commits between them.
    async fn record_at(
        &self,
        id: ProcessId,
    ) -> Result<Committed<Option<ProcessRecord>>, ServiceError> {
        self.store()
            .processes(move |repository| repository.process_at(id))
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
#[cfg(all(test, windows))]
mod windows_tests;
