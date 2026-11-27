//! Applying instance output lines and draining unread stdout of an ended instance from
//! its durable Process log before concluding it.

use ma2a_core::service::{AgentId, Channel as DataChannel, ProcessId, ServiceError};
use ma2a_store::{
    AgentInstanceRecord, AgentOutput, AgentOutputApplied, AgentOutputEffect, ProcessRecord,
};

use super::{
    super::lines::{Line, Lines, effect},
    AgentManager, Linked,
};

/// Content bytes read per batch while draining an ended instance's log.
const DRAIN_BATCH: usize = 262_144;

impl AgentManager {
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
