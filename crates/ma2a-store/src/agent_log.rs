//! Transaction helpers shared by every Agent task change: the bounded event log, the
//! instance outbox, and terminal transitions.

use ma2a_core::service::{
    AgentEvent, AgentEventBody, AgentId, AgentTaskId, AgentTaskReason, AgentTaskState,
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    AgentTaskRecord, StoreError,
    agent_rows::{TASK_COLUMNS, read_task},
};

/// Newest progress events kept per task; older ones are pruned.
pub const MAX_PROGRESS_EVENTS: u32 = 64;
/// Most state, message, and artifact events one task may record; the next fails it.
pub const MAX_DURABLE_EVENTS: u32 = 256;

/// What one outbox delivery carries.
#[expect(
    clippy::exhaustive_enums,
    reason = "delivery kinds are a closed durable vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentDelivery {
    /// Assign the task to the instance.
    Task(AgentTaskId),
    /// Deliver the user message recorded as this event.
    Message(AgentTaskId, u32),
    /// Ask the instance to cancel the task.
    Cancel(AgentTaskId),
    /// Deliver the result of this tool call.
    ToolResult(AgentTaskId, String),
}

impl AgentDelivery {
    const fn columns(&self) -> (u8, &AgentTaskId, Option<u32>, Option<&str>) {
        match self {
            Self::Task(task) => (0, task, None, None),
            Self::Message(task, seq) => (1, task, Some(*seq), None),
            Self::Cancel(task) => (2, task, None, None),
            Self::ToolResult(task, call) => (3, task, None, Some(call.as_str())),
        }
    }
}

pub(crate) fn load_task(
    transaction: &Transaction<'_>,
    id: AgentTaskId,
) -> Result<Option<AgentTaskRecord>, StoreError> {
    transaction
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM agent_tasks WHERE task_id = ?1"),
            [id.as_bytes().as_slice()],
            read_task,
        )
        .optional()?
        .transpose()
}

pub(crate) fn reload_task(
    transaction: &Transaction<'_>,
    id: AgentTaskId,
) -> Result<AgentTaskRecord, StoreError> {
    load_task(transaction, id)?.ok_or(StoreError::SchemaMismatch {
        detail: "a changed Agent task could not be read back",
    })
}

/// Appends one event and returns its sequence number; progress beyond the newest few is
/// pruned.
pub(crate) fn append_event(
    transaction: &Transaction<'_>,
    task: AgentTaskId,
    (body, at_ms): (AgentEventBody, u64),
) -> Result<u32, StoreError> {
    let last: u32 = transaction.query_row(
        "SELECT last_seq FROM agent_tasks WHERE task_id = ?1",
        [task.as_bytes().as_slice()],
        |row| row.get(0),
    )?;
    let seq = last.checked_add(1).ok_or(StoreError::SchemaMismatch {
        detail: "Agent task event sequence exhausted",
    })?;
    let progress = body.is_progress();
    let event = AgentEvent::new(seq, at_ms, body).to_bytes()?;
    transaction.execute(
        "INSERT INTO agent_task_events (task_id, seq, progress, event) VALUES (?1, ?2, ?3, ?4)",
        params![task.as_bytes().as_slice(), seq, progress, event],
    )?;
    transaction.execute(
        "UPDATE agent_tasks SET last_seq = ?2 WHERE task_id = ?1",
        params![task.as_bytes().as_slice(), seq],
    )?;
    if progress {
        transaction.execute(
            "DELETE FROM agent_task_events WHERE task_id = ?1 AND progress = 1 AND seq NOT IN
             (SELECT seq FROM agent_task_events WHERE task_id = ?1 AND progress = 1
              ORDER BY seq DESC LIMIT ?2)",
            params![task.as_bytes().as_slice(), MAX_PROGRESS_EVENTS],
        )?;
    }
    Ok(seq)
}

/// Returns how many never-pruned events a task holds.
pub(crate) fn durable_events(
    transaction: &Transaction<'_>,
    task: AgentTaskId,
) -> Result<u32, StoreError> {
    Ok(transaction.query_row(
        "SELECT COUNT(*) FROM agent_task_events WHERE task_id = ?1 AND progress = 0",
        [task.as_bytes().as_slice()],
        |row| row.get(0),
    )?)
}

/// Moves a task to `state`, recording its state event; terminal states record the end.
pub(crate) fn set_state(
    transaction: &Transaction<'_>,
    (task, at_ms): (AgentTaskId, u64),
    (state, reason): (AgentTaskState, Option<AgentTaskReason>),
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE agent_tasks SET state = ?2, reason = ?3,
         started_at_ms = CASE WHEN ?2 = 1 AND started_at_ms IS NULL THEN ?4 ELSE started_at_ms END,
         ended_at_ms = CASE WHEN ?5 THEN ?4 ELSE NULL END
         WHERE task_id = ?1",
        params![
            task.as_bytes().as_slice(),
            state.code(),
            reason.map(AgentTaskReason::code),
            at_ms,
            state.is_terminal(),
        ],
    )?;
    append_event(
        transaction,
        task,
        (AgentEventBody::State(state, reason), at_ms),
    )?;
    Ok(())
}

/// Returns the generation of the Agent's live instance, if one is live.
pub(crate) fn live_generation(
    transaction: &Transaction<'_>,
    agent: &AgentId,
) -> Result<Option<u64>, StoreError> {
    Ok(transaction
        .query_row(
            "SELECT generation FROM agent_instances WHERE agent_id = ?1 AND ended_at_ms IS NULL",
            [agent.as_str()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Adds one delivery to a live instance's outbox and returns its sequence number.
pub(crate) fn enqueue(
    transaction: &Transaction<'_>,
    (agent, generation): (&AgentId, u64),
    delivery: &AgentDelivery,
) -> Result<u64, StoreError> {
    let seq: u64 = transaction.query_row(
        "SELECT next_seq FROM agent_instances WHERE agent_id = ?1 AND generation = ?2",
        params![agent.as_str(), generation],
        |row| row.get(0),
    )?;
    let (kind, task, ref_seq, call) = delivery.columns();
    transaction.execute(
        "INSERT INTO agent_outbox (agent_id, generation, seq, kind, task_id, ref_seq, call_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            agent.as_str(),
            generation,
            seq,
            kind,
            task.as_bytes().as_slice(),
            ref_seq,
            call
        ],
    )?;
    transaction.execute(
        "UPDATE agent_instances SET next_seq = next_seq + 1
         WHERE agent_id = ?1 AND generation = ?2",
        params![agent.as_str(), generation],
    )?;
    Ok(seq)
}

/// Delivers to the task's instance when that instance is still the live one.
pub(crate) fn deliver_to_holder(
    transaction: &Transaction<'_>,
    record: &AgentTaskRecord,
    delivery: &AgentDelivery,
) -> Result<Option<u64>, StoreError> {
    let live = live_generation(transaction, &record.summary.agent)?;
    match (record.generation, live) {
        (Some(held), Some(live)) if held == live => Ok(Some(enqueue(
            transaction,
            (&record.summary.agent, live),
            delivery,
        )?)),
        _ => Ok(None),
    }
}
