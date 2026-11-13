//! Agent output applied exactly once: each adapter message and the advance of its
//! instance's output cursor commit in one transaction.

use ma2a_core::service::{
    AgentEventBody, AgentId, AgentMessage, AgentTaskId, AgentTaskReason, AgentTaskState,
    MessageRole,
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    AgentTaskRecord, Committed, Repository, StoreError,
    agent_actions::record_durable_event,
    agent_log::{MAX_DURABLE_EVENTS, append_event, durable_events, load_task, set_state},
    agent_tools::record_tool_call,
    repository::increment_revision,
    service_exposures::current_revision,
};

/// Where one adapter message ends in its instance's output.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct AgentOutput {
    /// The Agent.
    pub agent: AgentId,
    /// The instance that wrote the message.
    pub generation: u64,
    /// The output position just past the message.
    pub cursor: u64,
    /// When it was applied, in Unix milliseconds.
    pub at_ms: u64,
}

/// What one adapter message asks for, already decoded and validated.
#[expect(
    clippy::exhaustive_enums,
    reason = "adapter effects are a closed vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentOutputEffect {
    /// Nothing: a malformed or unusable message only advances the cursor.
    Ignore,
    /// The instance is ready for tasks.
    Ready,
    /// The instance processed every delivery up to this sequence number.
    Ack(u64),
    /// The Agent moved a task it holds.
    State {
        /// The task.
        task: AgentTaskId,
        /// `running`, `waiting`, `completed`, `failed`, or `cancelled`.
        state: AgentTaskState,
        /// The message that goes with it: the prompt or the result.
        message: Option<AgentMessage>,
        /// The failure text of `failed`.
        error: Option<String>,
    },
    /// The Agent wrote a message to the submitter.
    Message(AgentTaskId, AgentMessage),
    /// The Agent reported progress.
    Progress(AgentTaskId, String),
    /// The Agent asked for a tool; `rejection` answers it when the task has too many open.
    ToolCall {
        /// The task.
        task: AgentTaskId,
        /// The Agent's call identifier.
        call: String,
        /// The tool name.
        name: String,
        /// The canonical request.
        request: Vec<u8>,
        /// The result to record at once when the call cannot run now.
        rejection: Vec<u8>,
    },
}

/// What applying one message did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every application outcome must be handled explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentOutputApplied {
    /// This message was applied before, or its instance is no longer current.
    Stale,
    /// Only the cursor moved: the message names nothing this instance may change.
    Ignored,
    /// The message changed the task, which is now as returned.
    Task(Box<AgentTaskRecord>),
    /// The instance state changed (`ready` or `ack`).
    Instance,
    /// A new tool call is recorded and must now run.
    ToolCall(AgentTaskId, String),
    /// The tool call was answered at once with its rejection.
    ToolRejected(AgentTaskId, String),
}

/// Returns the task when it belongs to the emitting Agent and its instance still holds it.
///
/// Generations of different Agents can be equal, so the Agent is part of the ownership check.
fn held(
    transaction: &Transaction<'_>,
    task: AgentTaskId,
    output: &AgentOutput,
) -> Result<Option<AgentTaskRecord>, StoreError> {
    Ok(load_task(transaction, task)?.filter(|record| {
        record.summary.agent == output.agent
            && record.generation == Some(output.generation)
            && matches!(
                record.summary.state,
                AgentTaskState::Running | AgentTaskState::Waiting
            )
    }))
}

fn apply_state(
    transaction: &Transaction<'_>,
    (record, at_ms): (&AgentTaskRecord, u64),
    (state, message, error): (AgentTaskState, Option<AgentMessage>, Option<String>),
) -> Result<bool, StoreError> {
    let task = record.summary.id;
    let reason = match state {
        AgentTaskState::Failed => Some(AgentTaskReason::AgentFailed),
        // An Agent may only confirm a cancellation that was requested.
        AgentTaskState::Cancelled => match record.cancel {
            Some((reason, _)) => Some(reason),
            None => return Ok(false),
        },
        _ => None,
    };
    if state == record.summary.state || !record.summary.state.permits(state) {
        return Ok(false);
    }
    if let Some(message) = message.filter(|message| message.role() == MessageRole::Agent) {
        if state == AgentTaskState::Completed {
            transaction.execute(
                "UPDATE agent_tasks SET result = ?2 WHERE task_id = ?1",
                params![task.as_bytes().as_slice(), message.to_bytes()?],
            )?;
        }
        let body = AgentEventBody::Message(message);
        if record_durable_event(transaction, task, (body, at_ms))?.is_none() {
            return Ok(true);
        }
    }
    // A state change is a durable event too: an Agent that only alternates between
    // running and waiting reaches the same bound as one that only sends messages.
    if !state.is_terminal() && durable_events(transaction, task)? >= MAX_DURABLE_EVENTS {
        let limit = (AgentTaskState::Failed, Some(AgentTaskReason::Limit));
        set_state(transaction, (task, at_ms), limit)?;
        return Ok(true);
    }
    if state == AgentTaskState::Failed {
        transaction.execute(
            "UPDATE agent_tasks SET error = ?2 WHERE task_id = ?1",
            params![task.as_bytes().as_slice(), error],
        )?;
    }
    set_state(transaction, (task, at_ms), (state, reason))?;
    Ok(true)
}

fn apply_effect(
    transaction: &Transaction<'_>,
    output: &AgentOutput,
    effect: AgentOutputEffect,
) -> Result<AgentOutputApplied, StoreError> {
    let (generation, at_ms) = (output.generation, output.at_ms);
    let changed = |task| {
        load_task(transaction, task)?
            .map(|record| AgentOutputApplied::Task(Box::new(record)))
            .ok_or(StoreError::SchemaMismatch {
                detail: "an applied Agent task could not be read back",
            })
    };
    match effect {
        AgentOutputEffect::Ignore => Ok(AgentOutputApplied::Ignored),
        AgentOutputEffect::Ready => {
            transaction.execute(
                "UPDATE agent_instances SET ready = 1 WHERE agent_id = ?1 AND generation = ?2",
                params![output.agent.as_str(), generation],
            )?;
            Ok(AgentOutputApplied::Instance)
        }
        AgentOutputEffect::Ack(seq) => {
            transaction.execute(
                "UPDATE agent_instances SET acked_seq = ?3 WHERE agent_id = ?1 AND generation = ?2
                 AND acked_seq < ?3 AND ?3 < next_seq",
                params![output.agent.as_str(), generation, seq],
            )?;
            // An acknowledgement of something never sent acknowledges nothing.
            transaction.execute(
                "DELETE FROM agent_outbox WHERE agent_id = ?1 AND generation = ?2 AND seq <= ?3
                 AND ?3 < (SELECT next_seq FROM agent_instances
                           WHERE agent_id = ?1 AND generation = ?2)",
                params![output.agent.as_str(), generation, seq],
            )?;
            Ok(AgentOutputApplied::Instance)
        }
        AgentOutputEffect::State {
            task,
            state,
            message,
            error,
        } => match held(transaction, task, output)? {
            Some(record)
                if apply_state(transaction, (&record, at_ms), (state, message, error))? =>
            {
                changed(task)
            }
            _ => Ok(AgentOutputApplied::Ignored),
        },
        AgentOutputEffect::Message(task, message) => {
            if message.role() != MessageRole::Agent || held(transaction, task, output)?.is_none() {
                return Ok(AgentOutputApplied::Ignored);
            }
            let body = AgentEventBody::Message(message);
            record_durable_event(transaction, task, (body, at_ms))?;
            changed(task)
        }
        AgentOutputEffect::Progress(task, text) => {
            if held(transaction, task, output)?.is_none() {
                return Ok(AgentOutputApplied::Ignored);
            }
            transaction.execute(
                "UPDATE agent_tasks SET progress = ?2 WHERE task_id = ?1",
                params![task.as_bytes().as_slice(), text],
            )?;
            append_event(transaction, task, (AgentEventBody::Progress(text), at_ms))?;
            changed(task)
        }
        AgentOutputEffect::ToolCall {
            task,
            call,
            name,
            request,
            rejection,
        } => held(transaction, task, output)?.map_or(Ok(AgentOutputApplied::Ignored), |record| {
            let call = (call, name, request, rejection);
            record_tool_call(transaction, (&record, at_ms), call)
        }),
    }
}

impl Repository {
    /// Applies one adapter message and advances its instance's output cursor with it.
    ///
    /// A message at or before the stored cursor, or from an instance that is no longer the
    /// live one, changes nothing. A message whose effect would break a durable bound is
    /// ignored: only the cursor moves, so one such line can never stop every later one.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn apply_agent_output(
        &mut self,
        output: &AgentOutput,
        effect: AgentOutputEffect,
    ) -> Result<Committed<AgentOutputApplied>, StoreError> {
        match self.apply_output_once(output, effect) {
            // A constraint fails the same way on every retry; the change rolled back.
            Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                self.apply_output_once(output, AgentOutputEffect::Ignore)
            }
            applied => applied,
        }
    }

    fn apply_output_once(
        &mut self,
        output: &AgentOutput,
        effect: AgentOutputEffect,
    ) -> Result<Committed<AgentOutputApplied>, StoreError> {
        let transaction = self.immediate()?;
        let cursor: Option<u64> = transaction
            .query_row(
                "SELECT output_cursor FROM agent_instances WHERE agent_id = ?1 AND generation = ?2
                 AND ended_at_ms IS NULL",
                params![output.agent.as_str(), output.generation],
                |row| row.get(0),
            )
            .optional()?;
        if cursor.is_none_or(|cursor| cursor >= output.cursor) {
            let revision = current_revision(&transaction)?;
            transaction.commit()?;
            return Ok(Committed::new(revision, AgentOutputApplied::Stale));
        }
        let applied = apply_effect(&transaction, output, effect)?;
        transaction.execute(
            "UPDATE agent_instances SET output_cursor = ?3 WHERE agent_id = ?1 AND generation = ?2",
            params![output.agent.as_str(), output.generation, output.cursor],
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, applied))
    }
}
