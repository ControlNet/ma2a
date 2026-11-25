//! Agent task reads, the instance outbox, and task retention.

use std::collections::BTreeSet;

use ma2a_core::{
    EndpointId,
    service::{AgentEvent, AgentId, AgentTaskId, ArtifactRef},
};
use rusqlite::params;

use crate::{
    AgentDelivery, AgentRecord, AgentTaskRecord, Committed, Repository, StoreError,
    agent_log::load_task,
    agent_rows::{AGENT_COLUMNS, TASK_COLUMNS, read_agent, read_task},
    agents::load_agent,
    repository::increment_revision,
    service_exposures::current_revision,
};

/// One task, when it exists, with something more read about it, at the revision of that read.
type TaskRead<T> = Result<Committed<Option<(AgentTaskRecord, T)>>, StoreError>;

/// The number of unfinished tasks of `agent`.
pub(crate) fn live_task_count(
    connection: &rusqlite::Connection,
    agent: &AgentId,
) -> Result<usize, StoreError> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM agent_tasks WHERE agent_id = ?1 AND state BETWEEN 0 AND 2",
        [agent.as_str()],
        |row| row.get(0),
    )?)
}

/// How long a finished task is retained, in milliseconds: seven days.
pub const AGENT_TASK_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// Which tasks a listing returns.
#[derive(Clone, Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct AgentTaskFilter {
    /// Only this creator's tasks, or every creator's.
    pub creator: Option<EndpointId>,
    /// Only this Agent's tasks, or every Agent's.
    pub agent: Option<AgentId>,
    /// Only tasks whose identifier sorts after this one.
    pub after: Option<AgentTaskId>,
    /// Most rows returned.
    pub limit: u32,
}

/// One unacknowledged delivery and its sequence number.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "a delivery is exactly its sequence number and content"
)]
pub struct AgentOutboxEntry {
    /// Instance-scoped sequence number.
    pub seq: u64,
    /// What it delivers.
    pub delivery: AgentDelivery,
}

const MALFORMED: StoreError = StoreError::SchemaMismatch {
    detail: "persisted Agent outbox row is malformed",
};

impl Repository {
    /// Returns one Agent and its current instance.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed.
    pub fn agent(&self, agent: &AgentId) -> Result<Option<AgentRecord>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let record = load_agent(&transaction, agent)?;
        transaction.commit()?;
        Ok(record)
    }

    /// Returns every Agent in name order with its number of unfinished tasks, and the revision
    /// of the same read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agents(&self) -> Result<Committed<Vec<(AgentRecord, usize)>>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = current_revision(&transaction)?;
        let records = {
            let mut statement = transaction.prepare(&format!(
                "SELECT {AGENT_COLUMNS} FROM agents a
                 LEFT JOIN agent_instances i ON i.agent_id = a.agent_id ORDER BY a.agent_id"
            ))?;
            let rows = statement.query_map([], read_agent)?;
            rows.map(|row| {
                let record = row??;
                let live = live_task_count(&transaction, &record.definition.id)?;
                Ok((record, live))
            })
            .collect::<Result<Vec<_>, StoreError>>()?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, records))
    }

    /// Lists tasks in identifier order with the revision of the same read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_tasks(
        &self,
        filter: &AgentTaskFilter,
    ) -> Result<Committed<Vec<AgentTaskRecord>>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = current_revision(&transaction)?;
        let records = {
            let mut statement = transaction.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM agent_tasks
                 WHERE (?1 IS NULL OR creator_endpoint_id = ?1) AND (?2 IS NULL OR agent_id = ?2)
                 AND (?3 IS NULL OR task_id > ?3) ORDER BY task_id LIMIT ?4"
            ))?;
            let rows = statement.query_map(
                params![
                    filter.creator.map(|creator| creator.as_bytes().to_vec()),
                    filter.agent.as_ref().map(AgentId::as_str),
                    filter.after.map(|after| after.as_bytes().to_vec()),
                    filter.limit,
                ],
                read_task,
            )?;
            rows.map(|row| row?).collect::<Result<Vec<_>, _>>()?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, records))
    }

    /// Returns one task with its artifacts in publication order, and the revision of the same
    /// read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_task_details(&self, task: AgentTaskId) -> TaskRead<Vec<ArtifactRef>> {
        self.task_read(task, |repository| repository.agent_artifacts(task))
    }

    /// Returns one task with at most `limit` of its retained events after `after`, and the
    /// revision of the same read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_task_events_at(
        &self,
        task: AgentTaskId,
        page: (u32, u32),
    ) -> TaskRead<Vec<AgentEvent>> {
        self.task_read(task, |repository| repository.agent_task_events(task, page))
    }

    /// Reads one task and `more` about it in one transaction, with its revision.
    fn task_read<T>(
        &self,
        task: AgentTaskId,
        more: impl FnOnce(&Self) -> Result<T, StoreError>,
    ) -> TaskRead<T> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = current_revision(&transaction)?;
        let read = match load_task(&transaction, task)? {
            Some(record) => Some((record, more(self)?)),
            None => None,
        };
        transaction.commit()?;
        Ok(Committed::new(revision, read))
    }

    /// Returns every unfinished task, oldest first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn live_agent_tasks(&self) -> Result<Vec<AgentTaskRecord>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM agent_tasks WHERE state BETWEEN 0 AND 2
             ORDER BY created_at_ms, task_id"
        ))?;
        let rows = statement.query_map([], read_task)?;
        rows.map(|row| row?).collect()
    }

    /// Returns at most `limit` retained events after `after`, in order.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_task_events(
        &self,
        task: AgentTaskId,
        (after, limit): (u32, u32),
    ) -> Result<Vec<AgentEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT event FROM agent_task_events WHERE task_id = ?1 AND seq > ?2
             ORDER BY seq LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![task.as_bytes().as_slice(), after, limit], |row| {
                row.get::<_, Vec<u8>>(0)
            })?;
        rows.map(|row| Ok(AgentEvent::from_bytes(&row?)?)).collect()
    }

    /// Returns a task's artifacts in publication order.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_artifacts(&self, task: AgentTaskId) -> Result<Vec<ArtifactRef>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT name, media_type, size, blake3 FROM agent_artifacts WHERE task_id = ?1
             ORDER BY position",
        )?;
        let rows = statement.query_map([task.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (name, media, size, hash) = row?;
            let hash = <[u8; 32]>::try_from(hash.as_slice()).map_err(|_| MALFORMED)?;
            Ok(ArtifactRef::new((&name, &media), (size, hash))?)
        })
        .collect()
    }

    /// Returns the deliveries one instance has not acknowledged, in order.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_outbox(
        &self,
        (agent, generation): (&AgentId, u64),
    ) -> Result<Vec<AgentOutboxEntry>, StoreError> {
        self.agent_outbox_after((agent, generation), 0)
    }

    /// Returns the instance's unacknowledged deliveries after sequence number `after`.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn agent_outbox_after(
        &self,
        (agent, generation): (&AgentId, u64),
        after: u64,
    ) -> Result<Vec<AgentOutboxEntry>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT seq, kind, task_id, ref_seq, call_id FROM agent_outbox
             WHERE agent_id = ?1 AND generation = ?2 AND seq > ?3 ORDER BY seq",
        )?;
        let rows = statement.query_map(params![agent.as_str(), generation, after], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<u32>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (seq, kind, task, ref_seq, call) = row?;
            let task = AgentTaskId::try_from(task.as_slice())?;
            let delivery = match (kind, ref_seq, call) {
                (0, None, None) => AgentDelivery::Task(task),
                (1, Some(seq), None) => AgentDelivery::Message(task, seq),
                (2, None, None) => AgentDelivery::Cancel(task),
                (3, None, Some(call)) => AgentDelivery::ToolResult(task, call),
                _ => return Err(MALFORMED),
            };
            Ok(AgentOutboxEntry { seq, delivery })
        })
        .collect()
    }

    /// Returns the hash of every artifact any retained task references.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn referenced_artifact_hashes(&self) -> Result<BTreeSet<[u8; 32]>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT blake3 FROM agent_artifacts")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|row| <[u8; 32]>::try_from(row?.as_slice()).map_err(|_| MALFORMED))
            .collect()
    }

    /// Removes every task that finished longer ago than the retention period.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn expire_agent_tasks(&mut self, now_ms: u64) -> Result<Committed<usize>, StoreError> {
        let transaction = self.immediate()?;
        let removed = transaction.execute(
            "DELETE FROM agent_tasks WHERE state BETWEEN 3 AND 5 AND ended_at_ms + ?1 <= ?2",
            params![AGENT_TASK_RETENTION_MS, now_ms],
        )?;
        let revision = if removed > 0 {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, removed))
    }
}
