//! What a submitter or the Runtime does to a task after it exists: more messages,
//! cancellation, and published artifacts.

use ma2a_core::service::{
    AgentEvent, AgentEventBody, AgentMessage, AgentTaskId, AgentTaskReason, AgentTaskState,
    ArtifactRef, MAX_TASK_ARTIFACTS, MessageRole,
};
use rusqlite::{Transaction, params};

use crate::{
    AgentTaskRecord, Committed, Repository, StoreError,
    agent_log::{
        AgentDelivery, MAX_DURABLE_EVENTS, append_event, deliver_to_holder, durable_events,
        load_task, reload_task, set_state,
    },
    repository::increment_revision,
    service_exposures::current_revision,
};

/// What a task action did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every task action outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTaskChange {
    /// The action was recorded; the task as it now is.
    Changed(Box<AgentTaskRecord>),
    /// The task has already ended; nothing changed.
    Ended(Box<AgentTaskRecord>),
    /// The task already held as many durable events as it may, so instead of recording the
    /// action this failed the task; the task as it now is.
    Failed(Box<AgentTaskRecord>),
    /// The task cannot take this action now: a bound is reached, or the name is taken.
    Refused,
    /// No such task exists.
    Missing,
}

pub(crate) fn is_user_message(
    transaction: &Transaction<'_>,
    (task, seq): (AgentTaskId, u32),
) -> Result<bool, StoreError> {
    let event: Vec<u8> = transaction.query_row(
        "SELECT event FROM agent_task_events WHERE task_id = ?1 AND seq = ?2",
        params![task.as_bytes().as_slice(), seq],
        |row| row.get(0),
    )?;
    Ok(matches!(
        AgentEvent::from_bytes(&event)?.body,
        AgentEventBody::Message(message) if message.role() == MessageRole::User
    ))
}

/// Records one never-pruned event, or fails the task when it already holds the most.
pub(crate) fn record_durable_event(
    transaction: &Transaction<'_>,
    task: AgentTaskId,
    (body, at_ms): (AgentEventBody, u64),
) -> Result<Option<u32>, StoreError> {
    if durable_events(transaction, task)? >= MAX_DURABLE_EVENTS {
        crate::agent_tools::fail_at_limit(transaction, (task, at_ms))?;
        return Ok(None);
    }
    append_event(transaction, task, (body, at_ms)).map(Some)
}

fn finish(
    transaction: Transaction<'_>,
    (task, changed): (AgentTaskId, bool),
    change: impl FnOnce(Box<AgentTaskRecord>) -> AgentTaskChange,
) -> Result<Committed<AgentTaskChange>, StoreError> {
    let revision = if changed {
        increment_revision(&transaction)?
    } else {
        current_revision(&transaction)?
    };
    let record = reload_task(&transaction, task)?;
    transaction.commit()?;
    Ok(Committed::new(revision, change(Box::new(record))))
}

impl Repository {
    /// Records one more message from the submitter and delivers it to the task's instance.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn send_agent_message(
        &mut self,
        task: AgentTaskId,
        (message, at_ms): (&AgentMessage, u64),
    ) -> Result<Committed<AgentTaskChange>, StoreError> {
        let transaction = self.immediate()?;
        let Some(record) = load_task(&transaction, task)? else {
            let revision = current_revision(&transaction)?;
            return Ok(Committed::new(revision, AgentTaskChange::Missing));
        };
        if record.summary.state.is_terminal() || record.cancel.is_some() {
            return finish(transaction, (task, false), AgentTaskChange::Ended);
        }
        let body = AgentEventBody::Message(message.clone());
        let Some(seq) = record_durable_event(&transaction, task, (body, at_ms))? else {
            // Past the event bound the message was dropped and the task failed instead.
            return finish(transaction, (task, true), AgentTaskChange::Failed);
        };
        deliver_to_holder(&transaction, &record, &AgentDelivery::Message(task, seq))?;
        finish(transaction, (task, true), AgentTaskChange::Changed)
    }

    /// Cancels a task: a queued one at once, a live one by asking its instance first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn request_agent_cancel(
        &mut self,
        task: AgentTaskId,
        (reason, at_ms): (AgentTaskReason, u64),
    ) -> Result<Committed<AgentTaskChange>, StoreError> {
        let transaction = self.immediate()?;
        let Some(record) = load_task(&transaction, task)? else {
            let revision = current_revision(&transaction)?;
            return Ok(Committed::new(revision, AgentTaskChange::Missing));
        };
        let changed = match record.summary.state {
            state if state.is_terminal() => {
                return finish(transaction, (task, false), AgentTaskChange::Ended);
            }
            AgentTaskState::Queued => {
                let cancelled = (AgentTaskState::Cancelled, Some(reason));
                set_state(&transaction, (task, at_ms), cancelled)?;
                true
            }
            _ if record.cancel.is_some() => false,
            _ => {
                transaction.execute(
                    "UPDATE agent_tasks SET cancel_reason = ?2, cancel_requested_at_ms = ?3
                     WHERE task_id = ?1",
                    params![task.as_bytes().as_slice(), reason.code(), at_ms],
                )?;
                deliver_to_holder(&transaction, &record, &AgentDelivery::Cancel(task))?;
                true
            }
        };
        finish(transaction, (task, changed), AgentTaskChange::Changed)
    }

    /// Ends every task whose instance did not confirm its cancellation within `grace_ms`.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn expire_agent_cancels(
        &mut self,
        (now_ms, grace_ms): (u64, u64),
    ) -> Result<Committed<Vec<AgentTaskId>>, StoreError> {
        let transaction = self.immediate()?;
        let due: Vec<(Vec<u8>, u8)> = {
            let mut statement = transaction.prepare(
                "SELECT task_id, cancel_reason FROM agent_tasks WHERE state IN (1, 2)
                 AND cancel_requested_at_ms IS NOT NULL AND cancel_requested_at_ms + ?1 <= ?2",
            )?;
            let rows = statement.query_map(params![grace_ms, now_ms], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let mut ended = Vec::with_capacity(due.len());
        for (id, reason) in due {
            let task = AgentTaskId::try_from(id.as_slice())?;
            let reason = AgentTaskReason::from_code(reason.into())?;
            // The instance already has the cancellation; only the calls it still waits for
            // need answers.
            if let Some(record) = load_task(&transaction, task)? {
                crate::agent_tools::answer_open_calls(&transaction, (&record, now_ms))?;
            }
            let cancelled = (AgentTaskState::Cancelled, Some(reason));
            set_state(&transaction, (task, now_ms), cancelled)?;
            ended.push(task);
        }
        let revision = if ended.is_empty() {
            current_revision(&transaction)?
        } else {
            increment_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, ended))
    }

    /// Records one artifact a live task published; names are unique within a task.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn add_agent_artifact(
        &mut self,
        task: AgentTaskId,
        (artifact, at_ms): (&ArtifactRef, u64),
    ) -> Result<Committed<AgentTaskChange>, StoreError> {
        add_artifact(self.immediate()?, (task, None), (artifact, at_ms))
    }

    /// Adds one artifact for a tool call, and records that call's answer in the same commit
    /// when the artifact is added.
    ///
    /// A crash can then never separate the publication from its answer: a published artifact
    /// is never published again by the same call after a restart. A refused publication
    /// leaves the call pending for its runner to answer.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn publish_agent_artifact(
        &mut self,
        (task, call): (AgentTaskId, ToolAnswer<'_>),
        (artifact, at_ms): (&ArtifactRef, u64),
    ) -> Result<Committed<AgentTaskChange>, StoreError> {
        add_artifact(self.immediate()?, (task, Some(call)), (artifact, at_ms))
    }
}

/// One tool call's identifier and the answer to record for it.
pub type ToolAnswer<'a> = (&'a str, &'a [u8]);

fn add_artifact(
    transaction: Transaction<'_>,
    (task, call): (AgentTaskId, Option<ToolAnswer<'_>>),
    (artifact, at_ms): (&ArtifactRef, u64),
) -> Result<Committed<AgentTaskChange>, StoreError> {
    let Some(record) = load_task(&transaction, task)? else {
        let revision = current_revision(&transaction)?;
        return Ok(Committed::new(revision, AgentTaskChange::Missing));
    };
    if record.summary.state.is_terminal() {
        return finish(transaction, (task, false), AgentTaskChange::Ended);
    }
    let (count, taken): (usize, bool) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(SUM(name = ?2), 0) > 0 FROM agent_artifacts
         WHERE task_id = ?1",
        params![task.as_bytes().as_slice(), artifact.name()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if taken || count >= MAX_TASK_ARTIFACTS {
        return finish(transaction, (task, false), |_| AgentTaskChange::Refused);
    }
    // The event comes first: a task it fails at its bound publishes nothing.
    let body = AgentEventBody::Artifact(artifact.clone());
    if record_durable_event(&transaction, task, (body, at_ms))?.is_none() {
        return finish(transaction, (task, true), AgentTaskChange::Failed);
    }
    transaction.execute(
        "INSERT INTO agent_artifacts (task_id, name, position, media_type, size, blake3)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task.as_bytes().as_slice(),
            artifact.name(),
            count,
            artifact.media_type(),
            artifact.size(),
            artifact.blake3().as_slice(),
        ],
    )?;
    if let Some((call, result)) = call {
        crate::agent_tools::complete_call(&transaction, (task, call), (result, at_ms))?;
    }
    finish(transaction, (task, true), AgentTaskChange::Changed)
}
