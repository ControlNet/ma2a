//! Bounded FIFO admission queue for maintenance instance startups and disabled-Agent cleanup.

use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use ma2a_core::{
    RequestId,
    service::{AgentId, ProcessId},
};
use ma2a_store::AgentRecord;
use tokio::time::Instant;

use super::AgentManager;

/// FIFO admission queue for Agents waiting on one of `start_permits`.
#[derive(Debug, Default)]
pub(super) struct StartQueue {
    order: VecDeque<AgentId>,
    members: HashSet<AgentId>,
}

impl StartQueue {
    fn retain(&mut self, needed: &HashSet<AgentId>) {
        self.order.retain(|agent| needed.contains(agent));
        self.members.retain(|agent| needed.contains(agent));
    }

    fn push_back(&mut self, agent: AgentId) {
        if self.members.insert(agent.clone()) {
            self.order.push_back(agent);
        }
    }

    fn push_front(&mut self, agent: AgentId) {
        if self.members.insert(agent.clone()) {
            self.order.push_front(agent);
        }
    }

    fn pop_front(&mut self) -> Option<AgentId> {
        let agent = self.order.pop_front()?;
        self.members.remove(&agent);
        Some(agent)
    }
}

/// Whether `record`'s last instance still owns a non-terminal local Process (bound or unbound).
pub(super) fn has_surviving_predecessor(
    record: &AgentRecord,
    (active_ids, active_bindings): (&HashSet<ProcessId>, &HashSet<RequestId>),
) -> bool {
    let Some(instance) = record.instance else {
        return false;
    };
    if let Some(process) = instance.process_id {
        return active_ids.contains(&process);
    }
    super::super::instance::start_request(
        &record.definition.id,
        (record.times.0, instance.generation),
    )
    .is_ok_and(|request_id| active_bindings.contains(&request_id))
}

impl AgentManager {
    /// Prunes Agents that no longer need startup or cleanup and appends newly eligible ones.
    pub(super) fn enqueue_starts(&self, starts: &Arc<Mutex<StartQueue>>, pending: Vec<AgentId>) {
        let needed: HashSet<AgentId> = pending.iter().cloned().collect();
        let now = Instant::now();
        let mut queue = starts.lock().unwrap_or_else(PoisonError::into_inner);
        let coordination = self.coordination();
        queue.retain(&needed);
        for agent in pending {
            let waiting = coordination
                .restarts
                .get(&agent)
                .is_some_and(|(not_before, _)| now < *not_before);
            if !waiting && !coordination.starting.contains(&agent) {
                queue.push_back(agent);
            }
        }
    }

    /// Acquires permits for queued eligible Agents in FIFO order.
    fn take_admitted(
        &self,
        starts: &Arc<Mutex<StartQueue>>,
    ) -> Vec<(AgentId, tokio::sync::OwnedSemaphorePermit)> {
        let now = Instant::now();
        let mut admitted = Vec::new();
        let mut queue = starts.lock().unwrap_or_else(PoisonError::into_inner);
        let mut coordination = self.coordination();
        while let Some(agent) = queue.pop_front() {
            let waiting = coordination
                .restarts
                .get(&agent)
                .is_some_and(|(not_before, _)| now < *not_before);
            if waiting || coordination.starting.contains(&agent) {
                continue;
            }
            let Ok(permit) = Arc::clone(&self.0.start_permits).try_acquire_owned() else {
                queue.push_front(agent);
                break;
            };
            admitted.push((agent.clone(), permit));
            coordination.starting.insert(agent);
        }
        drop(coordination);
        drop(queue);
        admitted
    }

    /// Admits queued Agents in FIFO order up to `start_permits` and hands freed slots to
    /// waiting Agents as soon as a task finishes.
    pub(super) fn drain_starts(&self, starts: &Arc<Mutex<StartQueue>>) {
        if self.0.shutdown.is_cancelled() {
            return;
        }
        let admitted = self.take_admitted(starts);
        for (agent, permit) in admitted {
            let (manager, starts) = (self.clone(), Arc::clone(starts));
            self.0.tasks.spawn(async move {
                let shutdown = manager.0.shutdown.clone();
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = async {
                        if manager.reconcile_agent(&agent).await {
                            manager.assign(&agent).await;
                        }
                    } => {}
                }
                manager.coordination().starting.remove(&agent);
                drop(permit);
                manager.drain_starts(&starts);
            });
        }
    }

    /// Reconciles one Agent under its lifecycle lock: starts enabled instances or terminates
    /// a disabled Agent's surviving predecessor Process with backoff. Returns `true` when the
    /// Agent is enabled.
    async fn reconcile_agent(&self, agent: &AgentId) -> bool {
        let _lifecycle = self.lock_lifecycle(agent).await;
        let Ok(Some(record)) = self.agent(agent).await else {
            return false;
        };
        if !record.enabled {
            if self.previous_instance_terminal(&record).await {
                self.coordination().restarts.remove(agent);
            } else {
                self.schedule_backoff(agent, false);
            }
            return false;
        }
        self.ensure_instance_locked(agent).await;
        true
    }
}
