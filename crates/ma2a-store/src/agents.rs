//! Operator-defined Agents and their instances.
//!
//! A definition changes only while its Agent is stopped. Stopping an Agent fails every task
//! it has not finished; concluding an instance fails the tasks that instance held, while
//! queued tasks wait for the next instance.

use ma2a_core::service::{AgentId, AgentTaskId, AgentTaskReason, AgentTaskState, ProcessId};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    AgentDefinition, AgentRecord, Committed, Repository, StoreError,
    agent_log::set_state,
    agent_rows::{AGENT_COLUMNS, read_agent},
    repository::increment_revision,
    service_exposures::current_revision,
};

/// What defining an Agent did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every definition outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentDefine {
    /// The definition is stored; `true` when the Agent is new.
    Defined(Box<AgentRecord>, bool),
    /// The Agent is started; stop it before changing or removing it.
    Active,
    /// No such Agent exists.
    Missing,
}

pub(crate) fn load_agent(
    transaction: &Transaction<'_>,
    agent: &AgentId,
) -> Result<Option<AgentRecord>, StoreError> {
    transaction
        .query_row(
            &format!(
                "SELECT {AGENT_COLUMNS} FROM agents a
                 LEFT JOIN agent_instances i ON i.agent_id = a.agent_id WHERE a.agent_id = ?1"
            ),
            [agent.as_str()],
            read_agent,
        )
        .optional()?
        .transpose()
}

fn reload_agent(transaction: &Transaction<'_>, agent: &AgentId) -> Result<AgentRecord, StoreError> {
    load_agent(transaction, agent)?.ok_or(StoreError::SchemaMismatch {
        detail: "a changed Agent could not be read back",
    })
}

/// Fails every unfinished task of one Agent, or only those one instance generation holds.
pub(crate) fn fail_tasks(
    transaction: &Transaction<'_>,
    (agent, generation): (&AgentId, Option<u64>),
    (reason, at_ms): (AgentTaskReason, u64),
) -> Result<Vec<AgentTaskId>, StoreError> {
    let ids: Vec<Vec<u8>> = {
        let mut statement = transaction.prepare(
            "SELECT task_id FROM agent_tasks WHERE agent_id = ?1 AND state BETWEEN 0 AND 2
             AND (?2 IS NULL OR (generation = ?2 AND state IN (1, 2))) ORDER BY created_at_ms",
        )?;
        let rows = statement.query_map(params![agent.as_str(), generation], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut ended = Vec::with_capacity(ids.len());
    for id in ids {
        let task = AgentTaskId::try_from(id.as_slice())?;
        // A task whose cancellation was already requested ends as that cancellation.
        let cancel: Option<u8> = transaction.query_row(
            "SELECT cancel_reason FROM agent_tasks WHERE task_id = ?1",
            [id.as_slice()],
            |row| row.get(0),
        )?;
        let end = match cancel {
            Some(code) => (
                AgentTaskState::Cancelled,
                Some(AgentTaskReason::from_code(code.into())?),
            ),
            None => (AgentTaskState::Failed, Some(reason)),
        };
        set_state(transaction, (task, at_ms), end)?;
        ended.push(task);
    }
    Ok(ended)
}

impl Repository {
    /// Stores a new definition, or replaces the definition of a stopped Agent.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn define_agent(
        &mut self,
        definition: &AgentDefinition,
        at_ms: u64,
    ) -> Result<Committed<AgentDefine>, StoreError> {
        let transaction = self.immediate()?;
        let existing = load_agent(&transaction, &definition.id)?;
        if existing.as_ref().is_some_and(|record| record.enabled) {
            let revision = current_revision(&transaction)?;
            transaction.commit()?;
            return Ok(Committed::new(revision, AgentDefine::Active));
        }
        transaction.execute(
            "INSERT INTO agents (agent_id, description, start, tools, max_tasks, enabled,
             generation, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?6)
             ON CONFLICT (agent_id) DO UPDATE SET description = ?2, start = ?3, tools = ?4,
             max_tasks = ?5, updated_at_ms = ?6",
            params![
                definition.id.as_str(),
                definition.description,
                definition.start,
                definition.tools,
                definition.max_tasks,
                at_ms
            ],
        )?;
        let revision = increment_revision(&transaction)?;
        let record = reload_agent(&transaction, &definition.id)?;
        transaction.commit()?;
        Ok(Committed::new(
            revision,
            AgentDefine::Defined(Box::new(record), existing.is_none()),
        ))
    }

    /// Removes a stopped Agent's definition; its finished tasks stay until they expire.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn undefine_agent(
        &mut self,
        agent: &AgentId,
    ) -> Result<Committed<AgentDefine>, StoreError> {
        let transaction = self.immediate()?;
        let outcome = match load_agent(&transaction, agent)? {
            None => AgentDefine::Missing,
            Some(record) if record.enabled => AgentDefine::Active,
            Some(record) => {
                transaction.execute("DELETE FROM agents WHERE agent_id = ?1", [agent.as_str()])?;
                AgentDefine::Defined(Box::new(record), false)
            }
        };
        let revision = if matches!(outcome, AgentDefine::Defined(..)) {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, outcome))
    }

    /// Starts or stops an Agent; stopping fails every task it has not finished.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn set_agent_enabled(
        &mut self,
        agent: &AgentId,
        (enabled, at_ms): (bool, u64),
    ) -> Result<Committed<Option<AgentRecord>>, StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE agents SET enabled = ?2, updated_at_ms = ?3 WHERE agent_id = ?1 AND enabled != ?2",
            params![agent.as_str(), enabled, at_ms],
        )? > 0;
        if changed && !enabled {
            fail_tasks(
                &transaction,
                (agent, None),
                (AgentTaskReason::AgentStopped, at_ms),
            )?;
        }
        let revision = if changed {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        let record = load_agent(&transaction, agent)?;
        transaction.commit()?;
        Ok(Committed::new(revision, record))
    }

    /// Begins the next instance of a started Agent whose last instance was concluded.
    ///
    /// Returns `None` when the Agent is stopped, unknown, or already has a live instance.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn begin_agent_instance(
        &mut self,
        agent: &AgentId,
        at_ms: u64,
    ) -> Result<Committed<Option<AgentRecord>>, StoreError> {
        let transaction = self.immediate()?;
        let startable = load_agent(&transaction, agent)?.is_some_and(|record| {
            record.enabled && !record.instance.is_some_and(|instance| instance.is_live())
        });
        if !startable {
            let revision = current_revision(&transaction)?;
            transaction.commit()?;
            return Ok(Committed::new(revision, None));
        }
        transaction.execute(
            "UPDATE agents SET generation = generation + 1 WHERE agent_id = ?1",
            [agent.as_str()],
        )?;
        transaction.execute(
            "DELETE FROM agent_outbox WHERE agent_id = ?1",
            [agent.as_str()],
        )?;
        transaction.execute(
            "INSERT OR REPLACE INTO agent_instances (agent_id, generation, process_id, ready,
             output_cursor, next_seq, acked_seq, started_at_ms, ended_at_ms)
             SELECT agent_id, generation, NULL, 0, 0, 1, 0, ?2, NULL FROM agents WHERE agent_id = ?1",
            params![agent.as_str(), at_ms],
        )?;
        let revision = increment_revision(&transaction)?;
        let record = reload_agent(&transaction, agent)?;
        transaction.commit()?;
        Ok(Committed::new(revision, Some(record)))
    }

    /// Records the Process that hosts one live instance; it is set once.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn bind_agent_process(
        &mut self,
        (agent, generation): (&AgentId, u64),
        process: ProcessId,
    ) -> Result<Committed<bool>, StoreError> {
        let transaction = self.immediate()?;
        let bound = transaction.execute(
            "UPDATE agent_instances SET process_id = ?3 WHERE agent_id = ?1 AND generation = ?2
             AND process_id IS NULL AND ended_at_ms IS NULL",
            params![agent.as_str(), generation, process.as_bytes().as_slice()],
        )? > 0;
        let revision = if bound {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, bound))
    }

    /// Concludes one instance: the tasks it held fail as `agent_lost` and its outbox goes.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn end_agent_instance(
        &mut self,
        (agent, generation): (&AgentId, u64),
        at_ms: u64,
    ) -> Result<Committed<Vec<AgentTaskId>>, StoreError> {
        let transaction = self.immediate()?;
        let ended = transaction.execute(
            "UPDATE agent_instances SET ended_at_ms = max(?3, started_at_ms)
             WHERE agent_id = ?1 AND generation = ?2 AND ended_at_ms IS NULL",
            params![agent.as_str(), generation, at_ms],
        )? > 0;
        let (revision, tasks) = if ended {
            transaction.execute(
                "DELETE FROM agent_outbox WHERE agent_id = ?1",
                [agent.as_str()],
            )?;
            let tasks = fail_tasks(
                &transaction,
                (agent, Some(generation)),
                (AgentTaskReason::AgentLost, at_ms),
            )?;
            (increment_revision(&transaction)?, tasks)
        } else {
            (current_revision(&transaction)?, Vec::new())
        };
        transaction.commit()?;
        Ok(Committed::new(revision, tasks))
    }
}
