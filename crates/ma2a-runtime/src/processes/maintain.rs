//! Keeping hosted Processes consistent: recovery at start, withdrawal convergence after
//! every durable change, and retention.

use std::time::Duration;

use ma2a_core::service::{ProcessReason, ProcessState, ServiceAccess, ServiceError};
use ma2a_services::supervisor::SupervisorRequest;
use ma2a_store::ProcessRecord;

use super::{ProcessManager, credentials, follow::request};

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
    async fn active(&self) -> Vec<ProcessRecord> {
        self.store()
            .processes(|repository| repository.active_processes())
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    /// Re-adopts every non-terminal Process through its supervisor's proof, or decides it.
    pub(super) async fn reconcile(&self) {
        let active = self.active().await;
        self.release_startup();
        for record in active {
            if record.state == ProcessState::Created {
                let manager = self.clone();
                self.0
                    .tasks
                    .spawn(async move { manager.interrupt(&record).await });
            } else {
                self.follow(record.id, None);
            }
        }
    }

    /// A Process left `created` never got `go`: its supervisor is aborted if it is still
    /// waiting, and the start is recorded as interrupted either way.
    async fn interrupt(&self, record: &ProcessRecord) {
        let aborted = match credentials(record) {
            Some(credentials) => match self.address(record.id).connect(&credentials).await {
                Ok(channel) => request(channel, SupervisorRequest::Abort).await,
                Err(error) => Err(error),
            },
            None => Err(ServiceError::Internal),
        };
        let _recorded = match aborted {
            Ok(reported) => self.apply(record.id, reported).await,
            Err(_) => self.conclude(record).await,
        };
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
