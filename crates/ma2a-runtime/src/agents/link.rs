//! One link per live instance: output in, deliveries out.
//!
//! The link attaches to the instance's Process as its single stdin writer and reads output
//! from the instance's durable cursor. Each complete stdout line is applied in one Store
//! transaction together with the cursor just past it, so every Agent message takes effect
//! exactly once across daemon crashes. Every unacknowledged delivery is written again on
//! every new connection, after an empty line that ends any line a lost connection cut.

use std::time::Duration;

use ma2a_core::service::{
    AgentId, Channel as DataChannel, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessReason,
    ServiceError, decode_skipped,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    supervisor::{SupervisorStatus, protocol::decode_reading},
};
use ma2a_store::{AgentInstanceRecord, AgentOutput, AgentOutputApplied};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{AgentManager, lines::Lines, render};

/// How long a new instance has to announce `ready`.
const READY_DEADLINE_MS: u64 = 30_000;
/// Longest pause between attempts to reach an instance's supervisor.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// How one connection to an instance ended.
pub(super) enum Linked {
    /// The instance's Process ended.
    Ended,
    /// The connection broke; reach the supervisor again.
    Broken,
}

impl AgentManager {
    /// Starts the link of one live instance unless one already follows it.
    pub(super) fn link(&self, agent: &AgentId, generation: u64) {
        {
            let mut coordination = self.coordination();
            if coordination.links.get(agent) == Some(&generation) {
                return;
            }
            coordination.links.insert(agent.clone(), generation);
        }
        let (manager, agent) = (self.clone(), agent.clone());
        self.0.tasks.spawn(async move {
            let shutdown = manager.0.shutdown.clone();
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = manager.follow_instance(&agent, generation) => {}
            }
            let mut coordination = manager.coordination();
            if coordination.links.get(&agent) == Some(&generation) {
                coordination.links.remove(&agent);
            }
        });
    }

    async fn follow_instance(&self, agent: &AgentId, generation: u64) {
        let mut backoff = Duration::from_millis(250);
        loop {
            let Some(instance) = self.live_instance(agent, generation).await else {
                return;
            };
            let linked = match instance.process_id {
                Some(_) => self.connect(agent, instance).await,
                // The instance start is still in progress, or was interrupted by a crash.
                None => Linked::Broken,
            };
            match linked {
                Linked::Ended => {
                    self.conclude_instance(agent, &instance).await;
                    return;
                }
                Linked::Broken => {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// Runs one connection to the instance's supervisor.
    async fn connect(&self, agent: &AgentId, instance: AgentInstanceRecord) -> Linked {
        let Some(process) = instance.process_id else {
            return Linked::Broken;
        };
        let from = LogFrom::Cursor(instance.cursor);
        let Ok((_, (reader, mut writer), accepted)) =
            self.0.processes.attach_local(process, from).await
        else {
            return self.process_ended(process).await;
        };
        let Ok(((start, _, _), _)) = decode_reading(&accepted) else {
            return Linked::Broken;
        };
        let mut lines = Lines::new(instance.cursor);
        if start != instance.cursor {
            lines.skipped(start);
        }
        // An empty line first ends any delivery a lost connection cut short.
        if writer.data(DataChannel::Input, b"\n").await.is_err() {
            return Linked::Broken;
        }
        let output = self.read_output((agent, instance.generation), (reader, lines));
        let deliveries = self.write_deliveries((agent, instance.generation), &mut writer);
        let deadline = self.ready_deadline((agent, instance), process);
        tokio::select! {
            linked = output => linked,
            () = deliveries => Linked::Broken,
            () = deadline => Linked::Broken,
        }
    }

    /// Decides a Process the link cannot attach to: ended, or only unreachable now.
    async fn process_ended(&self, process: ma2a_core::service::ProcessId) -> Linked {
        match self.0.processes.local_record(process).await {
            Ok(Some(record)) if !record.state.is_terminal() => Linked::Broken,
            Ok(_) => Linked::Ended,
            Err(_) => Linked::Broken,
        }
    }

    async fn read_output<R: AsyncRead + Unpin>(
        &self,
        (agent, generation): (&AgentId, u64),
        (mut reader, mut lines): (FrameReader<R>, Lines),
    ) -> Linked {
        loop {
            let Ok(frame) = reader.expect().await else {
                return Linked::Broken;
            };
            let complete = match (frame.kind(), frame.data()) {
                (FrameKind::Data, Ok((DataChannel::Output, bytes))) => lines.output(bytes),
                (FrameKind::Data, Ok((_, bytes))) => {
                    lines.other(bytes.len());
                    Vec::new()
                }
                (FrameKind::Control, _) => match decode_skipped(frame.payload()) {
                    Ok(cursor) => {
                        lines.skipped(cursor);
                        Vec::new()
                    }
                    Err(_) => return Linked::Broken,
                },
                (FrameKind::Outcome, _) => return concluded(frame.payload()),
                _ => return Linked::Broken,
            };
            for (line, cursor) in complete {
                let effect = super::lines::effect(line.as_deref());
                let output = AgentOutput {
                    agent: agent.clone(),
                    generation,
                    cursor,
                    at_ms: self.now_ms(),
                };
                if self.apply_output(output, effect).await.is_err() {
                    return Linked::Broken;
                }
            }
        }
    }

    async fn apply_output(
        &self,
        output: AgentOutput,
        effect: ma2a_store::AgentOutputEffect,
    ) -> Result<(), ServiceError> {
        let agent = output.agent.clone();
        let applied = self
            .commit(move |repository| repository.apply_agent_output(&output, effect))
            .await?;
        match applied {
            AgentOutputApplied::Task(record) if record.summary.state.is_terminal() => {
                self.task_ended(record.summary.id);
                self.assign(&agent).await;
            }
            AgentOutputApplied::Instance => self.assign(&agent).await,
            AgentOutputApplied::ToolCall(task, call) => self.run_tool((task, call)),
            _ => {}
        }
        Ok(())
    }

    /// Writes every delivery the instance has not acknowledged, then each new one.
    async fn write_deliveries<W: AsyncWrite + Unpin>(
        &self,
        (agent, generation): (&AgentId, u64),
        writer: &mut FrameWriter<W>,
    ) {
        let mut changes = self.subscribe();
        let mut written = 0;
        loop {
            changes.mark_unchanged();
            let id = agent.clone();
            // Only deliveries not yet written are read and rendered again.
            let pending = self
                .store(move |repository| {
                    let entries = repository.agent_outbox_after((&id, generation), written)?;
                    render::lines(repository, &entries)
                })
                .await;
            let Ok(pending) = pending else {
                return;
            };
            let unsent: Vec<_> = pending
                .into_iter()
                .filter(|(seq, _)| *seq > written)
                .collect();
            for (seq, line) in unsent {
                if let Some(line) = line {
                    for chunk in line.chunks(MAX_DATA_CHUNK_LEN) {
                        if writer.data(DataChannel::Input, chunk).await.is_err() {
                            return;
                        }
                    }
                }
                written = seq;
            }
            if writer.flush().await.is_err() || changes.changed().await.is_err() {
                return;
            }
        }
    }

    /// Ends an instance that does not announce `ready` in time; never completes otherwise.
    async fn ready_deadline(
        &self,
        (agent, instance): (&AgentId, AgentInstanceRecord),
        process: ma2a_core::service::ProcessId,
    ) {
        if instance.ready {
            return std::future::pending().await;
        }
        let deadline = instance.started_ms.saturating_add(READY_DEADLINE_MS);
        let wait = deadline.saturating_sub(self.now_ms());
        tokio::time::sleep(Duration::from_millis(wait)).await;
        if self.overdue(agent, instance.generation).await {
            let reason = (1_000, ProcessReason::Requested);
            let _ended = self.0.processes.terminate((None, process), reason).await;
        }
        std::future::pending::<()>().await;
    }

    /// Whether instance `generation` of `agent` has still not announced `ready`.
    pub(super) async fn overdue(&self, agent: &AgentId, generation: u64) -> bool {
        !self
            .instance(agent)
            .await
            .is_some_and(|current| current.ready || current.generation != generation)
    }

    /// Instance `generation` of `agent` while it is still live.
    pub(super) async fn live_instance(
        &self,
        agent: &AgentId,
        generation: u64,
    ) -> Option<AgentInstanceRecord> {
        self.instance(agent)
            .await
            .filter(|instance| instance.generation == generation && instance.is_live())
    }

    /// The current instance of `agent`; a failed read is tried again, as it says nothing
    /// about the instance.
    async fn instance(&self, agent: &AgentId) -> Option<AgentInstanceRecord> {
        loop {
            match self.agent(agent).await {
                Ok(record) => return record.and_then(|record| record.instance),
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }
}

/// How a supervisor's `outcome` ends the link: for good once the Process ended. The supervisor
/// also answers a detach it decided on, as after a write the instance left waiting past the
/// stall bound; the link then connects again and writes what is unacknowledged once more.
pub(super) fn concluded(status: &[u8]) -> Linked {
    match SupervisorStatus::from_bytes(status) {
        Ok(status) if status.is_final() => Linked::Ended,
        _ => Linked::Broken,
    }
}
