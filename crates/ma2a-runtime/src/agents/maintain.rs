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
            _tick = sweep.tick() => {
                manager.converge().await;
                manager.run_pending_calls().await;
            }
            _tick = tick.tick() => {
                manager.instances().await;
                manager.expire_cancels().await;
                manager.release_ended().await;
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
        self.run_pending_calls().await;
    }

    /// Runs every recorded call that has no answer and is not running now. At start these are
    /// the calls the last daemon left; later, calls whose run ended early because a Store read
    /// failed.
    pub(super) async fn run_pending_calls(&self) {
        let pending = self
            .store(|repository| repository.pending_tool_calls())
            .await
            .unwrap_or_default();
        for call in pending {
            self.run_tool((call.task, call.call));
        }
    }

    /// Makes sure every started Agent has a linked instance, and assigns queued tasks.
    ///
    /// Missing or unbound instances start in bounded background tasks so slow supervisor
    /// bring-up never delays cancellation expiry or healthy Agents.
    async fn instances(&self) {
        let Ok((_, agents)) = self.list().await else {
            return;
        };
        for (record, _) in agents.into_iter().filter(|(record, _)| record.enabled) {
            let id = &record.definition.id;
            // A reserved name, started before such names were refused, can never run an
            // instance; it is stopped, so its tasks end and it takes no new ones.
            if super::operator::reserved_name(id.as_str()) {
                let _stopped = self.stop(id).await;
                continue;
            }
            if let Some(instance) = record
                .instance
                .filter(|instance| instance.is_live() && instance.process_id.is_some())
            {
                self.link(id, instance.generation);
                self.assign(id).await;
                continue;
            }
            self.start_instance(id);
        }
    }

    /// Starts or recovers one Agent's instance in a background task, at most once per Agent
    /// and bounded by `start_permits`.
    fn start_instance(&self, agent: &ma2a_core::service::AgentId) {
        let permit = {
            let mut coordination = self.coordination();
            let waiting = coordination
                .restarts
                .get(agent)
                .is_some_and(|(not_before, _)| tokio::time::Instant::now() < *not_before);
            if waiting || coordination.starting.contains(agent) {
                return;
            }
            let Ok(permit) = std::sync::Arc::clone(&self.0.start_permits).try_acquire_owned()
            else {
                return;
            };
            coordination.starting.insert(agent.clone());
            permit
        };
        let (manager, agent) = (self.clone(), agent.clone());
        self.0.tasks.spawn(async move {
            let shutdown = manager.0.shutdown.clone();
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = async {
                    manager.ensure_instance(&agent).await;
                    manager.assign(&agent).await;
                } => {}
            }
            manager.coordination().starting.remove(&agent);
            drop(permit);
        });
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

    /// Releases what a task held once it ended, however it ended: an Agent stop or a
    /// limit ends a task without passing through the paths that release it at once.
    async fn release_ended(&self) {
        let held: Vec<_> = self.coordination().listeners.keys().copied().collect();
        for task in held {
            let record = self
                .store(move |repository| repository.agent_task(task))
                .await;
            let ended = match record {
                Ok(Some(record)) => record.summary.state.is_terminal(),
                Ok(None) => true,
                Err(_) => false,
            };
            if ended {
                self.task_ended(task);
            }
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
