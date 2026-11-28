//! Applying instance output lines, draining unread stdout of an ended instance from
//! its durable Process log before concluding it, and enforcing readiness and stdin bounds.

use std::time::Duration;

use ma2a_core::service::{AgentId, Channel as DataChannel, ProcessId, ProcessReason, ServiceError};
use ma2a_store::{
    AgentInstanceRecord, AgentOutput, AgentOutputApplied, AgentOutputEffect, ProcessRecord,
};

use super::{
    super::lines::{Line, Lines, effect},
    AgentManager, INITIAL_BACKOFF, Linked, MAX_BACKOFF,
};

/// How long a new instance has to announce `ready`.
const READY_DEADLINE_MS: u64 = 30_000;
/// Content bytes read per batch while draining an ended instance's log.
const DRAIN_BATCH: usize = 262_144;

impl AgentManager {
    /// Ends an instance that does not announce `ready` in time; never returns.
    ///
    /// Runs across the whole generation (including connect and backoff) and retries termination
    /// with backoff while the same generation remains live and unready.
    pub(in crate::agents) async fn ready_deadline(
        &self,
        (agent, generation): (&AgentId, u64),
        terminated: &tokio::sync::watch::Sender<bool>,
    ) {
        let Some(instance) = self.live_instance(agent, generation).await else {
            return std::future::pending().await;
        };
        if !instance.ready {
            let deadline = instance.started_ms.saturating_add(READY_DEADLINE_MS);
            let wait = deadline
                .saturating_sub(self.now_ms())
                .min(READY_DEADLINE_MS);
            tokio::time::sleep(Duration::from_millis(wait)).await;
            let mut backoff = INITIAL_BACKOFF;
            while let Some(current) = self.live_instance(agent, generation).await {
                if current.ready {
                    break;
                }
                if let Some(process) = current.process_id {
                    if self
                        .terminate_with_retry((agent, generation), (process, true))
                        .await
                    {
                        let _sent = terminated.send(true);
                    }
                    break;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
        std::future::pending::<()>().await;
    }

    /// Retries terminating `process` with backoff while `generation` stays live (and unready
    /// when `only_if_unready`), returning whether the Process was confirmed ended or unknown.
    pub(super) async fn terminate_with_retry(
        &self,
        (agent, generation): (&AgentId, u64),
        (process, only_if_unready): (ProcessId, bool),
    ) -> bool {
        let mut backoff = INITIAL_BACKOFF;
        while if only_if_unready {
            self.overdue(agent, generation).await
        } else {
            self.live_instance(agent, generation).await.is_some()
        } {
            let reason = (1_000, ProcessReason::Requested);
            match self.0.processes.terminate((None, process), reason).await {
                Ok(committed) if committed.value().state.is_terminal() => return true,
                Err(ServiceError::UnknownProcess) => return true,
                _ => {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
        false
    }

    /// Decides a Process the link cannot attach to: ended, or only unreachable now.
    pub(super) async fn process_ended(
        &self,
        (agent, instance): (&AgentId, AgentInstanceRecord),
        process: ProcessId,
    ) -> Linked {
        match self.0.processes.local_record(process).await {
            Ok(Some(record)) if !record.state.is_terminal() => Linked::Broken,
            Ok(Some(record)) => self.drain_ended((agent, instance), &record).await,
            Ok(None) => Linked::Ended,
            Err(_) => Linked::Broken,
        }
    }

    /// Drains complete stdout lines past `instance.cursor` from `record`'s durable log
    /// before the instance is concluded.
    ///
    /// A transient log-read or Store-apply failure returns [`Linked::Broken`] so the link retries
    /// from the updated cursor instead of concluding early and failing tasks with `agent_lost`.
    async fn drain_ended(
        &self,
        (agent, instance): (&AgentId, AgentInstanceRecord),
        record: &ProcessRecord,
    ) -> Linked {
        let mut cursor = instance.cursor;
        let mut lines = Lines::new(cursor);
        loop {
            let read = match self
                .0
                .processes
                .read_local_log(record, (cursor, DRAIN_BATCH))
                .await
            {
                Ok(Some(read)) => read,
                Ok(None) => return Linked::Ended,
                Err(error @ (ServiceError::NotADirectory | ServiceError::IsADirectory)) => {
                    eprintln!(
                        "agent {agent} instance {} log is unusable ({error}); concluding",
                        instance.generation
                    );
                    return Linked::Ended;
                }
                Err(error) => {
                    eprintln!(
                        "agent {agent} instance {} log read failed ({error}); retrying",
                        instance.generation
                    );
                    return Linked::Broken;
                }
            };
            if read.start > cursor {
                lines.skipped(read.start);
            }
            if read.records.is_empty() {
                return Linked::Ended;
            }
            for (channel, bytes) in read.records {
                if channel == DataChannel::Output {
                    let complete = lines.output(&bytes);
                    if self
                        .apply_lines((agent, instance.generation), (complete, false))
                        .await
                        .is_err()
                    {
                        return Linked::Broken;
                    }
                } else {
                    lines.other(bytes.len());
                }
            }
            cursor = read.next;
        }
    }

    pub(super) async fn apply_lines(
        &self,
        (agent, generation): (&AgentId, u64),
        (complete, live): (Vec<Line>, bool),
    ) -> Result<(), ServiceError> {
        for (line, cursor) in complete {
            let output = AgentOutput {
                agent: agent.clone(),
                generation,
                cursor,
                at_ms: self.now_ms(),
            };
            self.apply_output(output, (effect(line.as_deref()), live))
                .await?;
        }
        Ok(())
    }

    async fn apply_output(
        &self,
        output: AgentOutput,
        (effect, live): (AgentOutputEffect, bool),
    ) -> Result<(), ServiceError> {
        let agent = output.agent.clone();
        let applied = self
            .commit(move |repository| repository.apply_agent_output(&output, effect))
            .await?;
        match applied {
            AgentOutputApplied::Task(record) if record.summary.state.is_terminal() => {
                self.task_ended(record.summary.id);
                if live {
                    self.assign(&agent).await;
                }
            }
            AgentOutputApplied::Instance if live => self.assign(&agent).await,
            AgentOutputApplied::ToolCall(task, call) if live => self.run_tool((task, call)),
            _ => {}
        }
        Ok(())
    }
}
