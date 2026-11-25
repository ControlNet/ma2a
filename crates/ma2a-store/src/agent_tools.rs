//! Tool calls an Agent makes for a task: recorded before they run and completed once, with
//! the result delivered to the instance that holds the task.

use ma2a_core::service::{AgentTaskId, AgentTaskReason, AgentTaskState};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    AgentOutputApplied, AgentTaskRecord, Committed, Repository, StoreError,
    agent_log::{AgentDelivery, deliver_to_holder, load_task, set_state},
    repository::increment_revision,
    service_exposures::current_revision,
};

/// Most tool calls one task may make; the next fails the task.
pub const MAX_TOOL_CALLS_PER_TASK: u32 = 256;
/// Most tool calls of one task running at once; the next is answered with its rejection.
pub const MAX_OPEN_TOOL_CALLS_PER_TASK: u32 = 8;

/// One recorded tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentToolCall {
    /// The task it was made for.
    pub task: AgentTaskId,
    /// The Agent's call identifier.
    pub call: String,
    /// The tool name.
    pub name: String,
    /// The canonical request.
    pub request: Vec<u8>,
    /// The result, once completed.
    pub result: Option<Vec<u8>>,
}

type CallRow = (Vec<u8>, String, String, Vec<u8>, Option<Vec<u8>>);

fn decode(row: CallRow) -> Result<AgentToolCall, StoreError> {
    let (task, call, name, request, result) = row;
    Ok(AgentToolCall {
        task: AgentTaskId::try_from(task.as_slice())?,
        call,
        name,
        request,
        result,
    })
}

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

/// The stored answer of every call a task's failure at one of its limits leaves open: the
/// `task_ended` tool error exactly as the Runtime encodes it.
pub const TASK_ENDED_ANSWER: &[u8] =
    br#"{"error":{"code":"task_ended","message":"the task has already ended"}}"#;

/// Records one tool call of a task the calling instance holds.
///
/// A call made while the task has too many open is answered at once with `rejection`. The
/// call past the task's limit never runs: it fails the task, which answers it with every call
/// still open (see [`fail_at_limit`]).
pub(crate) fn record_tool_call(
    transaction: &Transaction<'_>,
    (record, at_ms): (&AgentTaskRecord, u64),
    (call, name, request, rejection): (String, String, Vec<u8>, Vec<u8>),
) -> Result<AgentOutputApplied, StoreError> {
    let task = record.summary.id;
    // A repeated call identifier never runs twice, whatever it asks for.
    let Some((total, open)) = call_counts(transaction, task, &call)? else {
        return Ok(AgentOutputApplied::Ignored);
    };
    let limited = total >= MAX_TOOL_CALLS_PER_TASK;
    let rejected = !limited && open >= MAX_OPEN_TOOL_CALLS_PER_TASK;
    transaction.execute(
        "INSERT INTO agent_tool_calls (task_id, call_id, name, request, result, created_at_ms,
         completed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task.as_bytes().as_slice(),
            call,
            name,
            request,
            rejected.then_some(rejection.as_slice()),
            at_ms,
            rejected.then_some(at_ms),
        ],
    )?;
    if limited {
        fail_at_limit(transaction, (task, at_ms))?;
        return Ok(AgentOutputApplied::Task(Box::new(
            load_task(transaction, task)?.ok_or(StoreError::SchemaMismatch {
                detail: "a limited Agent task could not be read back",
            })?,
        )));
    }
    if rejected {
        deliver_to_holder(
            transaction,
            record,
            &AgentDelivery::ToolResult(task, call.clone()),
        )?;
        return Ok(AgentOutputApplied::ToolRejected(task, call));
    }
    Ok(AgentOutputApplied::ToolCall(task, call))
}

/// The calls a task made so far, and how many of them are open; `None` when `call` is one of
/// them.
fn call_counts(
    transaction: &Transaction<'_>,
    task: AgentTaskId,
    call: &str,
) -> Result<Option<(u32, u32)>, StoreError> {
    let (total, open, used): (u32, u32, bool) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(SUM(result IS NULL), 0), COALESCE(SUM(call_id = ?2), 0) > 0
         FROM agent_tool_calls WHERE task_id = ?1",
        params![task.as_bytes().as_slice(), call],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok((!used).then_some((total, open)))
}

/// Answers a tool call the instance makes for a task it held that has since ended with
/// [`TASK_ENDED_ANSWER`], so it is not left waiting for a result that never comes.
///
/// The call counts toward the task's call limit; past it, or for a call identifier already
/// used, the call is ignored. A task this instance never held is ignored too.
pub(crate) fn answer_ended_task_call(
    transaction: &Transaction<'_>,
    (task, output, at_ms): (AgentTaskId, &crate::AgentOutput, u64),
    (call, name, request): (String, String, Vec<u8>),
) -> Result<AgentOutputApplied, StoreError> {
    let Some(record) = load_task(transaction, task)?.filter(|record| {
        record.summary.agent == output.agent
            && record.generation == Some(output.generation)
            && record.summary.state.is_terminal()
    }) else {
        return Ok(AgentOutputApplied::Ignored);
    };
    match call_counts(transaction, task, &call)? {
        Some((total, _open)) if total < MAX_TOOL_CALLS_PER_TASK => {}
        _ => return Ok(AgentOutputApplied::Ignored),
    }
    transaction.execute(
        "INSERT INTO agent_tool_calls (task_id, call_id, name, request, result, created_at_ms,
         completed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            task.as_bytes().as_slice(),
            call,
            name,
            request,
            TASK_ENDED_ANSWER,
            at_ms
        ],
    )?;
    deliver_to_holder(
        transaction,
        &record,
        &AgentDelivery::ToolResult(task, call.clone()),
    )?;
    Ok(AgentOutputApplied::ToolRejected(task, call))
}

/// Fails a task at one of its limits.
///
/// Its open calls are answered (see [`answer_open_calls`]), and the instance holding it is
/// then sent the task's cancellation, so a handler waiting for a message that will never come
/// stops too.
pub(crate) fn fail_at_limit(
    transaction: &Transaction<'_>,
    (task, at_ms): (AgentTaskId, u64),
) -> Result<(), StoreError> {
    if let Some(record) = load_task(transaction, task)? {
        answer_open_calls(transaction, (&record, at_ms))?;
        deliver_to_holder(transaction, &record, &AgentDelivery::Cancel(task))?;
    }
    let limit = (AgentTaskState::Failed, Some(AgentTaskReason::Limit));
    set_state(transaction, (task, at_ms), limit)
}

/// Answers every open call of a task the Runtime is ending with [`TASK_ENDED_ANSWER`], oldest
/// first, and delivers each answer to the instance holding the task.
///
/// No result is delivered once a task ended, so that instance would otherwise wait for those
/// calls forever. A result that arrives later for one of them finds it answered and is dropped.
pub(crate) fn answer_open_calls(
    transaction: &Transaction<'_>,
    (record, at_ms): (&AgentTaskRecord, u64),
) -> Result<(), StoreError> {
    let task = record.summary.id;
    let mut statement = transaction.prepare(
        "SELECT call_id FROM agent_tool_calls WHERE task_id = ?1 AND result IS NULL
         ORDER BY created_at_ms, call_id",
    )?;
    let open = statement
        .query_map([task.as_bytes().as_slice()], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    transaction.execute(
        "UPDATE agent_tool_calls SET result = ?2, completed_at_ms = max(?3, created_at_ms)
         WHERE task_id = ?1 AND result IS NULL",
        params![task.as_bytes().as_slice(), TASK_ENDED_ANSWER, at_ms],
    )?;
    for call in open {
        deliver_to_holder(transaction, record, &AgentDelivery::ToolResult(task, call))?;
    }
    Ok(())
}

impl Repository {
    /// Records the result of one pending tool call and delivers it when the task is live.
    ///
    /// Returns whether this call completed it; a call already completed keeps its first
    /// result.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn complete_tool_call(
        &mut self,
        (task, call): (AgentTaskId, &str),
        (result, at_ms): (&[u8], u64),
    ) -> Result<Committed<bool>, StoreError> {
        let transaction = self.immediate()?;
        let completed = complete_call(&transaction, (task, call), (result, at_ms))?;
        let revision = if completed {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, completed))
    }

    /// Returns one tool call.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed.
    pub fn tool_call(
        &self,
        (task, call): (AgentTaskId, &str),
    ) -> Result<Option<AgentToolCall>, StoreError> {
        self.connection
            .query_row(
                "SELECT task_id, call_id, name, request, result FROM agent_tool_calls
                 WHERE task_id = ?1 AND call_id = ?2",
                params![task.as_bytes().as_slice(), call],
                read,
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    /// Returns every pending tool call of an unfinished task, oldest first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn pending_tool_calls(&self) -> Result<Vec<AgentToolCall>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT c.task_id, c.call_id, c.name, c.request, c.result FROM agent_tool_calls c
             JOIN agent_tasks t ON t.task_id = c.task_id
             WHERE c.result IS NULL AND t.state IN (1, 2) ORDER BY c.created_at_ms, c.call_id",
        )?;
        let rows = statement.query_map([], read)?;
        rows.map(|row| decode(row?)).collect()
    }
}

/// Records the result of one pending call and delivers it while its task is live; returns
/// whether this completed it.
pub(crate) fn complete_call(
    transaction: &Transaction<'_>,
    (task, call): (AgentTaskId, &str),
    (result, at_ms): (&[u8], u64),
) -> Result<bool, StoreError> {
    let completed = transaction.execute(
        "UPDATE agent_tool_calls SET result = ?3, completed_at_ms = max(?4, created_at_ms)
         WHERE task_id = ?1 AND call_id = ?2 AND result IS NULL",
        params![task.as_bytes().as_slice(), call, result, at_ms],
    )? > 0;
    if completed
        && let Some(record) =
            load_task(transaction, task)?.filter(|record| !record.summary.state.is_terminal())
    {
        let delivery = AgentDelivery::ToolResult(task, call.to_owned());
        deliver_to_holder(transaction, &record, &delivery)?;
    }
    Ok(completed)
}
