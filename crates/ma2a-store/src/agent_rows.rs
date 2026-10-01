//! Durable Agent, instance, and task records and their row decoding.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskDetails, AgentTaskId, AgentTaskReason, AgentTaskState,
        AgentTaskSummary, ArtifactRef, ProcessId,
    },
};
use rusqlite::Row;

use crate::StoreError;

const MALFORMED: StoreError = StoreError::SchemaMismatch {
    detail: "persisted Agent row is malformed",
};

/// What the operator defines for one Agent.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct AgentDefinition {
    /// The Agent's name.
    pub id: AgentId,
    /// Operator-written description; may be empty.
    pub description: String,
    /// Canonical Process start bytes of every instance.
    pub start: Vec<u8>,
    /// Allowed tool families as a bit set.
    pub tools: u8,
    /// Most tasks one instance works on at once.
    pub max_tasks: u8,
}

/// The current instance of one Agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "a Store row; views and tests name every field so none can be forgotten"
)]
pub struct AgentInstanceRecord {
    /// Which instance this is; every start takes the next number.
    pub generation: u64,
    /// The Process hosting it, once created.
    pub process_id: Option<ProcessId>,
    /// Whether the instance announced it is ready for tasks.
    pub ready: bool,
    /// Output position up to which the instance's messages were applied.
    pub cursor: u64,
    /// Sequence number the next delivery gets.
    pub next_seq: u64,
    /// Highest delivery the instance acknowledged.
    pub acked_seq: u64,
    /// When it started, in Unix milliseconds.
    pub started_ms: u64,
    /// When it was concluded, if it was.
    pub ended_ms: Option<u64>,
}

impl AgentInstanceRecord {
    /// Returns whether the instance may still receive tasks and deliveries.
    pub const fn is_live(&self) -> bool {
        self.ended_ms.is_none()
    }
}

/// One defined Agent and its current instance.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "a Store row; views and tests name every field so none can be forgotten"
)]
pub struct AgentRecord {
    /// What the operator defined.
    pub definition: AgentDefinition,
    /// Whether the operator has it started.
    pub enabled: bool,
    /// Instances ever started.
    pub generation: u64,
    /// Definition and last-change times in Unix milliseconds.
    pub times: (u64, u64),
    /// The current or last instance.
    pub instance: Option<AgentInstanceRecord>,
}

/// One task as the Store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentTaskRecord {
    /// The content-free view.
    pub summary: AgentTaskSummary,
    /// Fingerprint of the Agent and canonical submission.
    pub fingerprint: [u8; 32],
    /// The instance generation it is or was assigned to.
    pub generation: Option<u64>,
    /// The submitted message.
    pub input: AgentMessage,
    /// The inline result of a completed task.
    pub result: Option<AgentMessage>,
    /// The failure the Agent reported.
    pub error: Option<String>,
    /// The newest progress text.
    pub progress: Option<String>,
    /// Why and when a cancellation was requested.
    pub cancel: Option<(AgentTaskReason, u64)>,
}

impl AgentTaskRecord {
    /// Returns the full view with the task's artifacts.
    pub fn details(&self, artifacts: Vec<ArtifactRef>) -> AgentTaskDetails {
        let mut details = AgentTaskDetails::new(self.summary.clone(), self.input.clone());
        details.result.clone_from(&self.result);
        details.error.clone_from(&self.error);
        details.progress.clone_from(&self.progress);
        details.artifacts = artifacts;
        details
    }
}

pub(crate) const TASK_COLUMNS: &str = "task_id, agent_id, creator_endpoint_id, request_id, \
    fingerprint, from_agent, state, reason, generation, input, result, error, progress, \
    cancel_reason, cancel_requested_at_ms, last_seq, created_at_ms, started_at_ms, ended_at_ms";

pub(crate) const AGENT_COLUMNS: &str = "a.agent_id, a.description, a.start, a.tools, \
    a.max_tasks, a.enabled, a.generation, a.created_at_ms, a.updated_at_ms, i.generation, \
    i.process_id, i.ready, i.output_cursor, i.next_seq, i.acked_seq, i.started_at_ms, \
    i.ended_at_ms";

fn task_id(bytes: &[u8]) -> Result<AgentTaskId, StoreError> {
    AgentTaskId::try_from(bytes).map_err(|_| MALFORMED)
}

fn agent_id(text: &str) -> Result<AgentId, StoreError> {
    AgentId::new(text).map_err(|_| MALFORMED)
}

fn message(bytes: Option<Vec<u8>>) -> Result<Option<AgentMessage>, StoreError> {
    bytes
        .map(|bytes| AgentMessage::from_bytes(&bytes).map_err(|_| MALFORMED))
        .transpose()
}

pub(crate) fn read_task(row: &Row<'_>) -> rusqlite::Result<Result<AgentTaskRecord, StoreError>> {
    let id: Vec<u8> = row.get(0)?;
    let agent: String = row.get(1)?;
    let creator: Vec<u8> = row.get(2)?;
    let request: Vec<u8> = row.get(3)?;
    let fingerprint: Vec<u8> = row.get(4)?;
    let from_agent: Option<String> = row.get(5)?;
    let (state, reason): (u8, Option<u8>) = (row.get(6)?, row.get(7)?);
    let generation: Option<u64> = row.get(8)?;
    let input: Vec<u8> = row.get(9)?;
    let result: Option<Vec<u8>> = row.get(10)?;
    let notes: (Option<String>, Option<String>) = (row.get(11)?, row.get(12)?);
    let cancel: (Option<u8>, Option<u64>) = (row.get(13)?, row.get(14)?);
    let last_seq: u32 = row.get(15)?;
    let times: (u64, Option<u64>, Option<u64>) = (row.get(16)?, row.get(17)?, row.get(18)?);
    Ok((|| {
        let mut summary = AgentTaskSummary::new(
            (task_id(&id)?, agent_id(&agent)?),
            (
                EndpointId::try_from(creator.as_slice()).map_err(|_| MALFORMED)?,
                RequestId::try_from(request.as_slice()).map_err(|_| MALFORMED)?,
            ),
            times.0,
        );
        summary.from_agent = from_agent.as_deref().map(agent_id).transpose()?;
        summary.state = AgentTaskState::from_code(state.into()).map_err(|_| MALFORMED)?;
        summary.reason = reason
            .map(|reason| AgentTaskReason::from_code(reason.into()).map_err(|_| MALFORMED))
            .transpose()?;
        summary.times = times;
        summary.last_seq = last_seq;
        let cancel = match cancel {
            (Some(reason), Some(at_ms)) => Some((
                AgentTaskReason::from_code(reason.into()).map_err(|_| MALFORMED)?,
                at_ms,
            )),
            (None, None) => None,
            _ => return Err(MALFORMED),
        };
        Ok(AgentTaskRecord {
            summary,
            fingerprint: <[u8; 32]>::try_from(fingerprint.as_slice()).map_err(|_| MALFORMED)?,
            generation,
            input: message(Some(input))?.ok_or(MALFORMED)?,
            result: message(result)?,
            error: notes.0,
            progress: notes.1,
            cancel,
        })
    })())
}

pub(crate) fn read_agent(row: &Row<'_>) -> rusqlite::Result<Result<AgentRecord, StoreError>> {
    let id: String = row.get(0)?;
    let description: String = row.get(1)?;
    let start: Vec<u8> = row.get(2)?;
    let (tools, max_tasks, enabled): (u8, u8, bool) = (row.get(3)?, row.get(4)?, row.get(5)?);
    let generation: u64 = row.get(6)?;
    let times: (u64, u64) = (row.get(7)?, row.get(8)?);
    let instance_generation: Option<u64> = row.get(9)?;
    let process: Option<Vec<u8>> = row.get(10)?;
    let instance = match instance_generation {
        Some(generation) => Some((
            generation,
            process,
            (row.get::<_, bool>(11)?, row.get::<_, u64>(12)?),
            (row.get::<_, u64>(13)?, row.get::<_, u64>(14)?),
            (row.get::<_, u64>(15)?, row.get::<_, Option<u64>>(16)?),
        )),
        None => None,
    };
    Ok((|| {
        let instance = instance
            .map(
                |(
                    generation,
                    process,
                    (ready, cursor),
                    (next_seq, acked_seq),
                    (started, ended),
                )| {
                    Ok::<_, StoreError>(AgentInstanceRecord {
                        generation,
                        process_id: process
                            .map(|bytes| {
                                ProcessId::try_from(bytes.as_slice()).map_err(|_| MALFORMED)
                            })
                            .transpose()?,
                        ready,
                        cursor,
                        next_seq,
                        acked_seq,
                        started_ms: started,
                        ended_ms: ended,
                    })
                },
            )
            .transpose()?;
        Ok(AgentRecord {
            definition: AgentDefinition {
                id: agent_id(&id)?,
                description,
                start,
                tools,
                max_tasks,
            },
            enabled,
            generation,
            times,
            instance,
        })
    })())
}
