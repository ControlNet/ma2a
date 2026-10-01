//! Agent task creation and assignment: one `RequestId` binds exactly one task, and a task
//! is assigned to a ready instance in the same transaction as its delivery.

use ma2a_core::{
    EndpointId, RequestId,
    service::{AgentId, AgentMessage, AgentTaskId, AgentTaskState},
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    AgentTaskRecord, Committed, Repository, StoreError,
    agent_log::{
        AgentDelivery, MAX_UNACKED_DELIVERIES, enqueue, load_task, reload_task, set_state,
    },
    agents::load_agent,
    repository::increment_revision,
    service_exposures::current_revision,
};

/// Most unfinished tasks one creator may hold on this Runtime.
pub const MAX_LIVE_TASKS_PER_CREATOR: u32 = 16;
/// Most unfinished tasks on this Runtime.
pub const MAX_LIVE_TASKS: u32 = 256;
/// Most finished tasks retained per creator; older ones are removed first.
pub const MAX_RETAINED_TASKS_PER_CREATOR: u32 = 64;
/// Most finished tasks retained on this Runtime; older ones are removed first.
pub const MAX_RETAINED_TASKS: u32 = 1_024;

/// A task to record.
#[derive(Clone, Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct NewAgentTask {
    /// Fresh task identifier.
    pub id: AgentTaskId,
    /// The Agent it is submitted to.
    pub agent: AgentId,
    /// The submitting Endpoint and its `RequestId`.
    pub binding: (EndpointId, RequestId),
    /// Fingerprint of the Agent and canonical submission.
    pub fingerprint: [u8; 32],
    /// The submitting Agent, recorded for attribution only.
    pub from_agent: Option<AgentId>,
    /// The submitted message.
    pub input: AgentMessage,
    /// Creation time in Unix milliseconds.
    pub created_ms: u64,
}

/// What recording a submission did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every submission outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTaskCreation {
    /// A new task is queued.
    Created(Box<AgentTaskRecord>),
    /// This `RequestId` already created this task with the same submission.
    Existing(Box<AgentTaskRecord>),
    /// This `RequestId` already created a task from a different submission.
    Conflict,
    /// No such Agent is defined.
    UnknownAgent,
    /// The Agent is stopped.
    Unavailable,
    /// A task limit is reached; nothing was recorded.
    LimitExceeded,
}

fn count(
    transaction: &Transaction<'_>,
    terminal: bool,
    creator: Option<EndpointId>,
) -> Result<u32, StoreError> {
    let creator = creator.map(|creator| creator.as_bytes().to_vec());
    Ok(transaction.query_row(
        "SELECT COUNT(*) FROM agent_tasks WHERE (state BETWEEN 3 AND 5) = ?1
         AND (?2 IS NULL OR creator_endpoint_id = ?2)",
        params![terminal, creator],
        |row| row.get(0),
    )?)
}

/// Removes the oldest finished tasks beyond the retention bounds, binding and all.
fn make_room(transaction: &Transaction<'_>, creator: EndpointId) -> Result<(), StoreError> {
    for (scope, bound) in [
        (Some(creator), MAX_RETAINED_TASKS_PER_CREATOR),
        (None, MAX_RETAINED_TASKS),
    ] {
        let excess = count(transaction, true, scope)?.saturating_sub(bound);
        if excess > 0 {
            transaction.execute(
                "DELETE FROM agent_tasks WHERE task_id IN (SELECT task_id FROM agent_tasks
                 WHERE state BETWEEN 3 AND 5 AND (?1 IS NULL OR creator_endpoint_id = ?1)
                 ORDER BY ended_at_ms, task_id LIMIT ?2)",
                params![scope.map(|creator| creator.as_bytes().to_vec()), excess],
            )?;
        }
    }
    Ok(())
}

fn decide(
    transaction: &Transaction<'_>,
    new: &NewAgentTask,
) -> Result<Option<AgentTaskCreation>, StoreError> {
    let (creator, request_id) = new.binding;
    let existing: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT task_id FROM agent_tasks WHERE creator_endpoint_id = ?1 AND request_id = ?2",
            params![
                creator.as_bytes().as_slice(),
                request_id.as_bytes().as_slice()
            ],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let record = reload_task(transaction, AgentTaskId::try_from(id.as_slice())?)?;
        return Ok(Some(if record.fingerprint == new.fingerprint {
            AgentTaskCreation::Existing(Box::new(record))
        } else {
            AgentTaskCreation::Conflict
        }));
    }
    let decision = match load_agent(transaction, &new.agent)? {
        None => Some(AgentTaskCreation::UnknownAgent),
        Some(agent) if !agent.enabled => Some(AgentTaskCreation::Unavailable),
        Some(_)
            if count(transaction, false, Some(creator))? >= MAX_LIVE_TASKS_PER_CREATOR
                || count(transaction, false, None)? >= MAX_LIVE_TASKS =>
        {
            Some(AgentTaskCreation::LimitExceeded)
        }
        Some(_) => None,
    };
    Ok(decision)
}

impl Repository {
    /// Records a submission, or answers the task an earlier attempt with this `RequestId`
    /// made.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn create_agent_task(
        &mut self,
        new: &NewAgentTask,
    ) -> Result<Committed<AgentTaskCreation>, StoreError> {
        let transaction = self.immediate()?;
        if let Some(decision) = decide(&transaction, new)? {
            let revision = current_revision(&transaction)?;
            transaction.commit()?;
            return Ok(Committed::new(revision, decision));
        }
        let (creator, request_id) = new.binding;
        make_room(&transaction, creator)?;
        transaction.execute(
            "INSERT INTO agent_tasks (task_id, agent_id, creator_endpoint_id, request_id,
             fingerprint, from_agent, state, input, last_seq, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 0, ?8)",
            params![
                new.id.as_bytes().as_slice(),
                new.agent.as_str(),
                creator.as_bytes().as_slice(),
                request_id.as_bytes().as_slice(),
                new.fingerprint.as_slice(),
                new.from_agent.as_ref().map(AgentId::as_str),
                new.input.to_bytes()?,
                new.created_ms,
            ],
        )?;
        let queued = (AgentTaskState::Queued, None);
        set_state(&transaction, (new.id, new.created_ms), queued)?;
        let revision = increment_revision(&transaction)?;
        let record = reload_task(&transaction, new.id)?;
        transaction.commit()?;
        Ok(Committed::new(
            revision,
            AgentTaskCreation::Created(Box::new(record)),
        ))
    }

    /// Assigns queued tasks, oldest first, to the Agent's live ready instance while it has
    /// free slots; each assignment and its deliveries commit together.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn assign_agent_tasks(
        &mut self,
        agent: &AgentId,
        at_ms: u64,
    ) -> Result<Committed<Vec<AgentTaskId>>, StoreError> {
        let transaction = self.immediate()?;
        let instance = load_agent(&transaction, agent)?.and_then(|record| {
            let max = u32::from(record.definition.max_tasks);
            record
                .instance
                .filter(|instance| record.enabled && instance.is_live() && instance.ready)
                .map(|instance| (instance.generation, max))
        });
        let mut assigned = Vec::new();
        if let Some((generation, max_tasks)) = instance {
            // An instance that does not read what it is sent is given nothing more.
            let unacked: u32 = transaction.query_row(
                "SELECT COUNT(*) FROM agent_outbox WHERE agent_id = ?1 AND generation = ?2",
                params![agent.as_str(), generation],
                |row| row.get(0),
            )?;
            let max_tasks = if unacked >= MAX_UNACKED_DELIVERIES {
                0
            } else {
                max_tasks
            };
            let busy: u32 = transaction.query_row(
                "SELECT COUNT(*) FROM agent_tasks WHERE agent_id = ?1 AND generation = ?2
                 AND state IN (1, 2)",
                params![agent.as_str(), generation],
                |row| row.get(0),
            )?;
            let queued: Vec<Vec<u8>> = {
                let mut statement = transaction.prepare(
                    "SELECT task_id FROM agent_tasks WHERE agent_id = ?1 AND state = 0
                     ORDER BY created_at_ms, task_id LIMIT ?2",
                )?;
                let rows = statement.query_map(
                    params![agent.as_str(), max_tasks.saturating_sub(busy)],
                    |row| row.get(0),
                )?;
                rows.collect::<Result<_, _>>()?
            };
            for id in queued {
                let task = AgentTaskId::try_from(id.as_slice())?;
                assign(&transaction, (agent, generation), (task, at_ms))?;
                assigned.push(task);
            }
        }
        let revision = if assigned.is_empty() {
            current_revision(&transaction)?
        } else {
            increment_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, assigned))
    }

    /// Returns one task.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed.
    pub fn agent_task(&self, task: AgentTaskId) -> Result<Option<AgentTaskRecord>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let record = load_task(&transaction, task)?;
        transaction.commit()?;
        Ok(record)
    }
}

/// Moves one queued task to `running` on an instance and delivers it with every message
/// its submitter sent while it waited.
fn assign(
    transaction: &Transaction<'_>,
    (agent, generation): (&AgentId, u64),
    (task, at_ms): (AgentTaskId, u64),
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE agent_tasks SET generation = ?2 WHERE task_id = ?1",
        params![task.as_bytes().as_slice(), generation],
    )?;
    set_state(transaction, (task, at_ms), (AgentTaskState::Running, None))?;
    enqueue(transaction, (agent, generation), &AgentDelivery::Task(task))?;
    let pending: Vec<u32> = {
        let mut statement = transaction.prepare(
            "SELECT seq FROM agent_task_events WHERE task_id = ?1 AND progress = 0 ORDER BY seq",
        )?;
        let rows = statement.query_map([task.as_bytes().as_slice()], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for seq in pending {
        if crate::agent_actions::is_user_message(transaction, (task, seq))? {
            enqueue(
                transaction,
                (agent, generation),
                &AgentDelivery::Message(task, seq),
            )?;
        }
    }
    Ok(())
}
