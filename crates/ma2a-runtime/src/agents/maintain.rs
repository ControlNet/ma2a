//! Keeping Agents consistent: recovery at start, instances and cancellation grace every
//! second, withdrawal convergence after every durable change, and retention.

use std::time::Duration;

use ma2a_core::service::{AgentTaskReason, ServiceAccess, ServiceError};

use super::AgentManager;

/// How often instances, assignment, and cancellation grace are checked.
const TICK: Duration = Duration::from_secs(1);
/// How often convergence runs even without a durable change.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15);
/// How often ended tasks and unreferenced blobs are removed.
const EXPIRE_INTERVAL: Duration = Duration::from_secs(60);
/// How long an instance has to confirm a cancellation before the task ends anyway.
const CANCEL_GRACE_MS: u64 = 10_000;
/// Bound on links and tool calls observing shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Runs recovery, then maintenance until the Runtime stops.
///
/// Daemon shutdown never stops an instance: its Process keeps running under its
/// supervisor and is linked again at the next start.
pub(crate) async fn run(manager: AgentManager) {
    manager.reconcile().await;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
            _tick = tick.tick() => {
                manager.instances().await;
                manager.expire_cancels().await;
                let Ok(revision) = manager.0.store.revision().await else {
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

impl AgentManager {
    /// Links every live instance, starts every missing one, and re-runs every tool call
    /// that was recorded but never completed.
    pub(super) async fn reconcile(&self) {
        self.instances().await;
        let pending = self
            .store(|repository| repository.pending_tool_calls())
            .await
            .unwrap_or_default();
        for call in pending {
            self.run_tool((call.task, call.call));
        }
    }

    /// Makes sure every started Agent has a linked instance, and assigns queued tasks.
    async fn instances(&self) {
        let Ok((_, agents)) = self.list().await else {
            return;
        };
        for (record, _) in agents.into_iter().filter(|(record, _)| record.enabled) {
            self.ensure_instance(&record.definition.id).await;
            self.assign(&record.definition.id).await;
        }
    }

    /// Ends every task whose instance did not confirm its cancellation in time.
    async fn expire_cancels(&self) {
        let now_ms = self.now_ms();
        let ended = self
            .store(move |repository| repository.expire_agent_cancels((now_ms, CANCEL_GRACE_MS)))
            .await
            .map(ma2a_store::Committed::into_value)
            .unwrap_or_default();
        if !ended.is_empty() {
            self.changed();
        }
        for task in ended {
            self.task_ended(task);
        }
    }

    /// Cancels every task whose remote creator no longer has one complete authorization
    /// path to its Agent; the local Endpoint's own tasks depend on no grant.
    pub(super) async fn converge(&self) {
        let live = self
            .store(|repository| repository.live_agent_tasks())
            .await
            .unwrap_or_default();
        for record in live {
            let summary = &record.summary;
            if summary.creator == self.0.local || record.cancel.is_some() {
                continue;
            }
            let access = ServiceAccess::Agent(summary.agent.clone());
            let decision = self
                .0
                .store
                .authorize_service((self.0.local, summary.creator), access)
                .await;
            if decision.is_err_and(|error| error == ServiceError::Unauthorized) {
                let (task, at_ms) = (summary.id, self.now_ms());
                let reason = AgentTaskReason::Unauthorized;
                let _cancelled = self
                    .commit(move |repository| {
                        repository.request_agent_cancel(task, (reason, at_ms))
                    })
                    .await;
            }
        }
    }

    /// Removes tasks past their retention, then the blobs nothing references any more.
    pub(super) async fn expire(&self) {
        let now_ms = self.now_ms();
        let removed = self
            .store(move |repository| repository.expire_agent_tasks(now_ms))
            .await
            .map(ma2a_store::Committed::into_value)
            .unwrap_or_default();
        if removed > 0 {
            self.changed();
        }
        self.collect_blobs().await;
    }
}
