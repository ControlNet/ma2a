//! Instances: starting the next one, concluding the last, assignment, and restart backoff.

use std::time::Duration;

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentTaskId, ProcessId, ProcessReason, ProcessSpec, ProcessStart, ServiceError,
    },
};
use ma2a_store::{AgentDefinition, AgentInstanceRecord, AgentRecord};
use tokio::time::Instant;

use super::{AgentManager, fault, private_dir};

/// Shortest and longest pause before an ended instance's successor starts.
const BACKOFF: (Duration, Duration) = (Duration::from_secs(1), Duration::from_secs(60));
/// How often an unbound Process's release is tried, and the delay step between attempts.
const RELEASE_ATTEMPTS: u32 = 4;
const RELEASE_RETRY: Duration = Duration::from_millis(100);
/// Uptime after which an instance's end restarts the backoff from its shortest pause.
const STABLE_UPTIME_MS: u64 = 60_000;

/// Whether a Process launched for `generation` is the live, bound Process of `instance`.
///
/// An Agent undefined while its Process started has no instance, so nothing keeps that Process.
fn keeps(instance: Option<&AgentInstanceRecord>, (generation, process): (u64, ProcessId)) -> bool {
    instance.is_some_and(|instance| {
        instance.generation == generation
            && instance.is_live()
            && instance.process_id == Some(process)
    })
}

/// The `RequestId` of one instance's start, so a crash between the Process commit and the
/// instance commit can never start a second Process for the same generation.
///
/// A definition's creation time is part of it: an Agent undefined and defined again counts
/// its generations from zero, and must never find the Processes of the earlier one.
fn start_request(
    agent: &AgentId,
    (defined_at_ms, generation): (u64, u64),
) -> Result<RequestId, ServiceError> {
    let mut hasher = blake3::Hasher::new_derive_key("ma2a agent instance start v1");
    hasher.update(agent.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(&defined_at_ms.to_be_bytes());
    hasher.update(&generation.to_be_bytes());
    let hash = hasher.finalize();
    RequestId::try_from(hash.as_bytes().get(..16).unwrap_or_default())
        .map_err(|_| ServiceError::Internal)
}

impl AgentManager {
    /// Makes sure a started Agent has a linked live instance, starting the next one once
    /// its restart backoff has passed.
    pub(super) async fn ensure_instance(&self, agent: &AgentId) {
        let Ok(Some(record)) = self.agent(agent).await else {
            return;
        };
        if !record.enabled || super::operator::reserved_name(agent.as_str()) {
            return;
        }
        if let Some(instance) = record.instance.filter(AgentInstanceRecord::is_live) {
            if instance.process_id.is_none() {
                self.launch(&record, instance).await;
            }
            self.link(agent, instance.generation);
            return;
        }
        let waiting = self
            .coordination()
            .restarts
            .get(agent)
            .is_some_and(|(not_before, _)| Instant::now() < *not_before);
        if waiting {
            return;
        }
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let begun = self
            .commit(move |repository| repository.begin_agent_instance(&id, at_ms))
            .await;
        let Ok(Some(record)) = begun else {
            return;
        };
        let Some(instance) = record.instance else {
            return;
        };
        self.launch(&record, instance).await;
        self.link(agent, instance.generation);
    }

    /// Creates (or finds, after a crash) the Process of one instance and records it.
    async fn launch(&self, record: &AgentRecord, instance: AgentInstanceRecord) {
        let agent = &record.definition.id;
        let launched = async {
            let workspace = self.workspace(agent);
            private_dir(&workspace).map_err(|_| ServiceError::Unavailable)?;
            let start = instance_start(&record.definition, &workspace.to_string_lossy())?;
            let request_id = start_request(agent, (record.times.0, instance.generation))?;
            self.0.processes.start_local(request_id, start).await
        };
        match launched.await {
            Ok(process) => {
                let (id, generation) = (agent.clone(), instance.generation);
                let bound = self
                    .commit(move |repository| {
                        repository.bind_agent_process((&id, generation), process.id)
                    })
                    .await;
                if matches!(bound, Ok(false)) {
                    self.release_unbound(agent, (generation, process.id)).await;
                }
            }
            // Nothing could start: the instance ends now and its successor waits.
            Err(_) => self.conclude_instance(agent, &instance).await,
        }
    }

    /// Terminates a Process launched for an instance that ended before it was bound, as when
    /// the Agent was stopped meanwhile; one bound by a concurrent launch keeps running.
    ///
    /// Once the Agent is undefined nothing else will find this Process again, so a failed read
    /// or termination is retried a few times instead of being given up at once.
    async fn release_unbound(&self, agent: &AgentId, (generation, process): (u64, ProcessId)) {
        for attempt in 0..RELEASE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(RELEASE_RETRY * attempt).await;
            }
            // An unreadable record says nothing about the binding, so the Process is never
            // terminated on one: it might belong to a concurrent launch that bound it.
            let Ok(record) = self.agent(agent).await else {
                continue;
            };
            let instance = record.as_ref().and_then(|record| record.instance.as_ref());
            if keeps(instance, (generation, process)) {
                return;
            }
            let reason = (super::operator::STOP_GRACE_MS, ProcessReason::Requested);
            if self
                .0
                .processes
                .terminate((None, process), reason)
                .await
                .is_ok()
            {
                return;
            }
        }
        eprintln!("agent process could not be released after {RELEASE_ATTEMPTS} attempts");
    }

    /// Concludes an instance whose Process ended and schedules its successor.
    pub(super) async fn conclude_instance(&self, agent: &AgentId, instance: &AgentInstanceRecord) {
        let (id, generation, at_ms) = (agent.clone(), instance.generation, self.now_ms());
        let Ok(failed) = self
            .commit(move |repository| repository.end_agent_instance((&id, generation), at_ms))
            .await
        else {
            return;
        };
        for task in failed {
            self.task_ended(task);
        }
        let uptime = at_ms.saturating_sub(instance.started_ms);
        let mut coordination = self.coordination();
        let previous = coordination.restarts.get(agent).map(|(_, next)| *next);
        let pause = match previous {
            Some(next) if uptime < STABLE_UPTIME_MS => next,
            _ => BACKOFF.0,
        };
        let next = (pause * 2).min(BACKOFF.1);
        coordination
            .restarts
            .insert(agent.clone(), (Instant::now() + pause, next));
    }

    /// Assigns queued tasks to the Agent's ready instance, oldest first.
    pub(super) async fn assign(&self, agent: &AgentId) {
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let assigned = self
            .store(move |repository| repository.assign_agent_tasks(&id, at_ms))
            .await
            .map(ma2a_store::Committed::into_value);
        if assigned.is_ok_and(|assigned| !assigned.is_empty()) {
            fault("after_assigned");
            self.changed();
        }
    }

    /// Releases everything that lives only as long as one task.
    pub(super) fn task_ended(&self, task: AgentTaskId) {
        let listeners = self.coordination().listeners.remove(&task);
        for listener in listeners.into_iter().flatten() {
            listener.cancel();
        }
    }
}

/// Builds the start of one instance: the definition plus the adapter environment.
pub(super) fn instance_start(
    definition: &AgentDefinition,
    workspace: &str,
) -> Result<ProcessStart, ServiceError> {
    let invalid = |_| ServiceError::InvalidInput;
    let defined = ProcessStart::from_bytes(&definition.start).map_err(invalid)?;
    let base = defined.spec();
    let cwd = base.cwd().unwrap_or(workspace).to_owned();
    let program = base.program().map(str::to_owned);
    let mut spec = ProcessSpec::new(program, base.args().to_vec(), Some(cwd)).map_err(invalid)?;
    let adapter = [
        ("MA2A_AGENT_ID", definition.id.as_str()),
        ("MA2A_AGENT_WORKSPACE", workspace),
        ("MA2A_AGENT_PROTOCOL", "1"),
    ];
    let defined_env = base
        .env()
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()));
    for (name, value) in defined_env.chain(adapter) {
        spec = spec
            .with_env(name.to_owned(), value.to_owned())
            .map_err(invalid)?;
    }
    let start = ProcessStart::new(spec, true, defined.log_limit())
        .and_then(|start| start.with_label("ma2a.agent".to_owned(), definition.id.to_string()))
        .map_err(invalid)?;
    // The additions can outgrow the header frame the defined start still fit in.
    start.to_bytes().map_err(invalid)?;
    Ok(start)
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::AgentId;

    use ma2a_core::service::ProcessId;
    use ma2a_store::AgentInstanceRecord;

    use super::{keeps, start_request};

    fn instance(generation: u64, process_id: Option<ProcessId>) -> AgentInstanceRecord {
        AgentInstanceRecord {
            generation,
            process_id,
            ready: false,
            cursor: 0,
            next_seq: 0,
            acked_seq: 0,
            started_ms: 1_000,
            ended_ms: None,
        }
    }

    #[test]
    fn only_the_live_instance_that_bound_a_process_keeps_it() {
        let process = ProcessId::from_bytes([7; 16]);
        let other = ProcessId::from_bytes([8; 16]);
        assert!(keeps(Some(&instance(3, Some(process))), (3, process)));
        // Undefined meanwhile: no record and so no instance keeps the Process.
        assert!(!keeps(None, (3, process)));
        // Stopped before the bind: the instance never named a Process.
        assert!(!keeps(Some(&instance(3, None)), (3, process)));
        // A later generation, another Process, or a concluded instance does not keep it.
        assert!(!keeps(Some(&instance(4, Some(process))), (3, process)));
        assert!(!keeps(Some(&instance(3, Some(other))), (3, process)));
        let ended = AgentInstanceRecord {
            ended_ms: Some(2_000),
            ..instance(3, Some(process))
        };
        assert!(!keeps(Some(&ended), (3, process)));
    }

    #[test]
    fn instance_starts_are_distinct_per_definition_and_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let agent = AgentId::new("worker")?;
        let first = start_request(&agent, (1_000, 1))?;
        assert_eq!(first, start_request(&agent, (1_000, 1))?);
        assert_ne!(first, start_request(&agent, (1_000, 2))?);
        // The same generation of a later definition is another start.
        assert_ne!(first, start_request(&agent, (2_000, 1))?);
        Ok(())
    }
}
