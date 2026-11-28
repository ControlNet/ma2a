//! Keeping hosted Processes consistent: recovery at start, withdrawal convergence after
//! every durable change, and retention.

use std::time::Duration;

use ma2a_core::service::{ProcessId, ProcessReason, ProcessState, ServiceAccess, ServiceError};
use ma2a_services::supervisor::SupervisorRequest;
use ma2a_store::ProcessRecord;

use super::{ProcessManager, credentials, follow::request};

/// Initial and longest pause between retries of startup scan or `created` settlement.
const RECOVERY_BACKOFF: (Duration, Duration) =
    (Duration::from_millis(250), Duration::from_secs(10));

/// How a `created` Process that is no longer being brought up must be settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CreatedSettlement {
    /// Left `created` by a prior boot (or an uncommitted `starting`): abort any waiting
    /// supervisor and record `failed(interrupted)` or its final status.
    Interrupted,
    /// Supervisor launch failed and committing `failed(spawn_failed)` failed transiently.
    SpawnFailed,
}

async fn pause_recovery(backoff: Duration) -> Duration {
    tokio::time::sleep(backoff).await;
    (backoff * 2).min(RECOVERY_BACKOFF.1)
}

/// How often durable state is checked for a change that may withdraw consent.
const CONVERGE_INTERVAL: Duration = Duration::from_secs(1);
/// How often convergence runs even without a durable change, so a termination that failed
/// is retried and every live Process has a follower.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15);
/// How often ended Processes are archived and tombstones pruned.
const EXPIRE_INTERVAL: Duration = Duration::from_secs(60);
/// Grace period of a Process ended because its creator lost authorization.
const WITHDRAWN_GRACE_MS: u32 = 5_000;
/// Bound on followers observing shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Runs recovery, then convergence and retention until the Runtime stops.
///
/// Daemon shutdown never stops a Process: it only closes this Runtime's connections to
/// their supervisors, which keep running and are re-adopted at the next start.
pub(crate) async fn run(manager: ProcessManager) {
    manager.reconcile().await;
    let mut converge = tokio::time::interval(CONVERGE_INTERVAL);
    converge.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut expire = tokio::time::interval(EXPIRE_INTERVAL);
    expire.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sweep =
        tokio::time::interval_at(tokio::time::Instant::now() + SWEEP_INTERVAL, SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seen = None;
    loop {
        tokio::select! {
            biased;
            () = manager.0.shutdown.cancelled() => break,
            _tick = expire.tick() => manager.expire().await,
            _tick = sweep.tick() => manager.converge().await,
            _tick = converge.tick() => {
                let Ok(revision) = manager.store().revision().await else {
                    continue;
                };
                if seen != Some(revision) {
                    seen = Some(revision);
                    manager.converge().await;
                }
            }
        }
    }
    manager.0.tasks.close();
    let _drained = tokio::time::timeout(SHUTDOWN_GRACE, manager.0.tasks.wait()).await;
}

impl ProcessManager {
    async fn active_result(&self) -> Result<Vec<ProcessRecord>, ServiceError> {
        self.store()
            .processes(|repository| repository.active_processes())
            .await?
            .map_err(|_| ServiceError::Unavailable)
    }

    async fn active(&self) -> Vec<ProcessRecord> {
        self.active_result().await.unwrap_or_default()
    }

    /// Re-adopts every non-terminal Process through its supervisor's proof, or decides it.
    ///
    /// A transient Store failure during the startup scan is retried with bounded backoff
    /// while the admission fence stays closed, so prior-boot `created` rows are never
    /// abandoned or mixed with newly admitted starts.
    pub(super) async fn reconcile(&self) {
        let mut backoff = RECOVERY_BACKOFF.0;
        let active = loop {
            match self.active_result().await {
                Ok(active) => break active,
                Err(_) => {
                    tokio::select! {
                        () = self.0.shutdown.cancelled() => return,
                        next = pause_recovery(backoff) => backoff = next,
                    }
                }
            }
        };
        for record in active {
            if record.state == ProcessState::Created {
                self.settle_created((record.id, Some(record)), CreatedSettlement::Interrupted);
            } else {
                self.follow(record.id, None);
            }
        }
        self.release_startup();
    }

    /// Retries settling one `created` Process with bounded backoff until it is terminal or gone.
    pub(super) fn settle_created(
        &self,
        (id, initial): (ProcessId, Option<ProcessRecord>),
        settlement: CreatedSettlement,
    ) {
        if !self.coordination().settling.insert(id) {
            return;
        }
        let manager = self.clone();
        self.0.tasks.spawn(async move {
            let shutdown = manager.0.shutdown.clone();
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = manager.settle_created_loop((id, initial), settlement) => {}
            }
            manager.coordination().settling.remove(&id);
        });
    }

    async fn settle_created_loop(
        &self,
        (id, mut initial): (ProcessId, Option<ProcessRecord>),
        settlement: CreatedSettlement,
    ) {
        let mut backoff = RECOVERY_BACKOFF.0;
        loop {
            let record = match initial.take() {
                Some(record) => record,
                None => match self.record(id).await {
                    Ok(Some(record)) => record,
                    Ok(None) => return,
                    Err(_) => {
                        backoff = pause_recovery(backoff).await;
                        continue;
                    }
                },
            };
            if record.state != ProcessState::Created {
                return;
            }
            let result = match settlement {
                CreatedSettlement::Interrupted => self.interrupt(&record).await,
                CreatedSettlement::SpawnFailed => {
                    let failed = (ProcessState::Failed, Some(ProcessReason::SpawnFailed));
                    self.step(id, failed).await.map(|_| ())
                }
            };
            if result.is_ok() || result == Err(ServiceError::UnknownProcess) {
                return;
            }
            backoff = pause_recovery(backoff).await;
        }
    }

    /// A Process left `created` never got `go`: its supervisor is aborted if it is still
    /// waiting, and the start is recorded as interrupted either way.
    async fn interrupt(&self, record: &ProcessRecord) -> Result<(), ServiceError> {
        let Some(credentials) = credentials(record) else {
            return self.conclude(record).await.map(|_| ());
        };
        let aborted = match self.address(record.id).connect(&credentials).await {
            Ok(channel) => request(channel, SupervisorRequest::Abort).await,
            Err(error) => Err(error),
        };
        match aborted {
            Ok(reported) => self.apply(record.id, reported).await.map(|_| ()),
            Err(ServiceError::Unreachable | ServiceError::Unauthorized) => {
                self.conclude(record).await.map(|_| ())
            }
            Err(error) => Err(error),
        }
    }

    /// Ends every Process whose creator no longer has one complete authorization path, and
    /// follows every live Process that lost its follower.
    pub(super) async fn converge(&self) {
        for record in self.active().await {
            if record.state == ProcessState::Created {
                continue;
            }
            self.follow(record.id, None);
            // The local Endpoint's own Processes depend on no grant.
            if record.creator == self.0.local
                || self
                    .coordination()
                    .terminating
                    .get(&record.id)
                    .is_some_and(|entry| {
                        entry.reason == ProcessReason::Unauthorized && entry.in_flight > 0
                    })
            {
                continue;
            }
            let decision = self
                .store()
                .authorize_service((self.0.local, record.creator), ServiceAccess::Process)
                .await;
            if decision == Err(ServiceError::Unauthorized) {
                let manager = self.clone();
                self.0.tasks.spawn(async move {
                    let target = (None, record.id);
                    let reason = ProcessReason::Unauthorized;
                    let _ended = manager
                        .terminate(target, (WITHDRAWN_GRACE_MS, reason))
                        .await;
                });
            }
        }
    }

    /// Archives Processes that ended long ago and removes their output.
    pub(super) async fn expire(&self) {
        let now_ms = self.now_ms();
        let archived = self
            .store()
            .processes(move |repository| repository.expire_processes(now_ms))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        for id in archived {
            self.remove_directory(id).await;
        }
    }
}
