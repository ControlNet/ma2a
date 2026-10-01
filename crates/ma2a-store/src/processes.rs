//! Durable Processes: one `RequestId` binds exactly one Process, and only lifecycle-permitted
//! transitions are ever written.

use ma2a_core::{
    EndpointId, ProtocolError, RequestId,
    service::{ProcessId, ProcessReason, ProcessState, Termination},
};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::{
    Committed, Repository, StoreError,
    process_rows::{
        PROCESS_COLUMNS, ProcessRecord, decode_row, read_row, reason_code, state_code,
        termination_columns,
    },
    repository::increment_revision,
};

/// Most non-terminal Processes one creator may hold on this Runtime.
pub const MAX_ACTIVE_PROCESSES_PER_CREATOR: u32 = 8;
/// Most non-terminal Processes on this Runtime.
pub const MAX_ACTIVE_PROCESSES: u32 = 32;
/// Most non-archived Process records on this Runtime.
pub const MAX_RETAINED_PROCESSES: u32 = 128;
/// Most non-archived Process records one creator may hold, so that no member's ended
/// Processes can fill [`MAX_RETAINED_PROCESSES`] for every other member.
pub const MAX_RETAINED_PROCESSES_PER_CREATOR: u32 = 16;

/// SQL predicate of a record that still holds its start and output.
const RETAINED: &str = "archived_at_ms IS NULL";

/// SQL predicate of a non-terminal state.
const ACTIVE: &str = "state BETWEEN 0 AND 2";

/// A Process to record before any supervisor exists.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct NewProcess<'a> {
    /// Fresh Process identifier.
    pub id: ProcessId,
    /// The creating Endpoint and its start `RequestId`.
    pub binding: (EndpointId, RequestId),
    /// Fingerprint of the canonical start body.
    pub fingerprint: [u8; 32],
    /// Canonical start bytes and display name.
    pub start: (&'a [u8], Option<&'a str>),
    /// Supervisor ownership secret.
    pub secret: [u8; 32],
    /// Creation time in Unix milliseconds.
    pub created_ms: u64,
}

/// What recording a start did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every start outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessCreation {
    /// A new Process was recorded.
    Created(ProcessRecord),
    /// This `RequestId` already created this Process with the same body.
    Existing(ProcessRecord),
    /// This `RequestId` already created a Process with a different body.
    Conflict,
    /// A Process limit is reached; nothing was recorded.
    LimitExceeded,
}

/// One lifecycle step and its end detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct ProcessTransition {
    /// The next state; it must be permitted from the current one.
    pub to: ProcessState,
    /// Why it ended, for `terminated`, `failed`, and `lost`.
    pub reason: Option<ProcessReason>,
    /// How the child ended, when observed.
    pub termination: Option<Termination>,
    /// When the step happened, in Unix milliseconds.
    pub at_ms: u64,
}

/// Which Processes a listing returns.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers name every field so none can be forgotten"
)]
pub struct ProcessFilter {
    /// Only this creator's Processes, or every Process.
    pub creator: Option<EndpointId>,
    /// Only Processes whose identifier sorts after this one.
    pub after: Option<ProcessId>,
    /// Most rows returned.
    pub limit: u32,
    /// Whether archived tombstones are included.
    pub archived: bool,
}

/// What archiving a Process did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every archive outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessArchive {
    /// The Process is archived; its output may now be removed.
    Archived(Box<ProcessRecord>),
    /// The Process has not ended yet; nothing changed.
    Active,
    /// No such Process exists.
    Missing,
}

pub(crate) fn load(
    transaction: &Transaction<'_>,
    id: ProcessId,
) -> Result<Option<ProcessRecord>, StoreError> {
    transaction
        .query_row(
            &format!("SELECT {PROCESS_COLUMNS} FROM processes WHERE process_id = ?1"),
            [id.as_bytes().as_slice()],
            read_row,
        )
        .optional()?
        .map(decode_row)
        .transpose()
}

fn count(
    transaction: &Transaction<'_>,
    predicate: &str,
    creator: Option<EndpointId>,
) -> Result<u32, StoreError> {
    let sql = format!(
        "SELECT COUNT(*) FROM processes WHERE {predicate} AND (?1 IS NULL OR creator_endpoint_id = ?1)"
    );
    let creator = creator.map(|creator| creator.as_bytes().to_vec());
    Ok(transaction.query_row(&sql, [creator], |row| row.get(0))?)
}

impl Repository {
    /// Records a start, or answers the Process an earlier attempt with this `RequestId` made.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn create_process(
        &mut self,
        new: &NewProcess<'_>,
    ) -> Result<Committed<ProcessCreation>, StoreError> {
        let transaction = self.immediate()?;
        let (creator, request_id) = new.binding;
        let existing = transaction
            .query_row(
                &format!(
                    "SELECT {PROCESS_COLUMNS} FROM processes
                     WHERE creator_endpoint_id = ?1 AND request_id = ?2"
                ),
                params![
                    creator.as_bytes().as_slice(),
                    request_id.as_bytes().as_slice()
                ],
                read_row,
            )
            .optional()?
            .map(decode_row)
            .transpose()?;
        let creation = if let Some(record) = existing {
            if record.fingerprint == new.fingerprint {
                ProcessCreation::Existing(record)
            } else {
                ProcessCreation::Conflict
            }
        } else if count(&transaction, ACTIVE, Some(creator))? >= MAX_ACTIVE_PROCESSES_PER_CREATOR
            || count(&transaction, ACTIVE, None)? >= MAX_ACTIVE_PROCESSES
            || count(&transaction, RETAINED, Some(creator))? >= MAX_RETAINED_PROCESSES_PER_CREATOR
            || count(&transaction, RETAINED, None)? >= MAX_RETAINED_PROCESSES
        {
            ProcessCreation::LimitExceeded
        } else {
            let (start, name) = new.start;
            transaction.execute(
                "INSERT INTO processes (process_id, creator_endpoint_id, request_id, fingerprint,
                 start, name, state, secret, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
                params![
                    new.id.as_bytes().as_slice(),
                    creator.as_bytes().as_slice(),
                    request_id.as_bytes().as_slice(),
                    new.fingerprint.as_slice(),
                    start,
                    name,
                    new.secret.as_slice(),
                    new.created_ms,
                ],
            )?;
            increment_revision(&transaction)?;
            ProcessCreation::Created(load(&transaction, new.id)?.ok_or(
                StoreError::SchemaMismatch {
                    detail: "a recorded Process could not be read back",
                },
            )?)
        };
        let revision = crate::service_exposures::current_revision(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, creation))
    }

    /// Moves one Process forward; any transition its lifecycle does not permit is refused.
    ///
    /// # Errors
    /// Returns `Protocol(NOT_FOUND)` for an unknown Process and `Protocol(CONFLICT)` for a
    /// transition the lifecycle forbids; nothing changes in either case.
    pub fn transition_process(
        &mut self,
        id: ProcessId,
        transition: ProcessTransition,
    ) -> Result<Committed<ProcessRecord>, StoreError> {
        let transaction = self.immediate()?;
        let current =
            load(&transaction, id)?.ok_or(StoreError::Protocol(ProtocolError::NOT_FOUND))?;
        if !current.state.permits(transition.to) {
            return Err(StoreError::Protocol(ProtocolError::CONFLICT));
        }
        let terminal = transition.to.is_terminal();
        let (kind, value) = termination_columns(transition.termination);
        transaction.execute(
            "UPDATE processes SET state = ?2, reason = ?3, termination_kind = ?4,
             termination_value = ?5,
             started_at_ms = CASE WHEN ?2 = 2 THEN ?6 ELSE started_at_ms END,
             ended_at_ms = CASE WHEN ?7 THEN ?6 ELSE NULL END,
             secret = CASE WHEN ?7 THEN NULL ELSE secret END
             WHERE process_id = ?1",
            params![
                id.as_bytes().as_slice(),
                state_code(transition.to),
                transition.reason.map(reason_code),
                kind,
                value,
                transition.at_ms,
                terminal,
            ],
        )?;
        let revision = increment_revision(&transaction)?;
        let record = load(&transaction, id)?.ok_or(StoreError::SchemaMismatch {
            detail: "a transitioned Process could not be read back",
        })?;
        transaction.commit()?;
        Ok(Committed::new(revision, record))
    }

    /// Returns one Process, archived or not.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row is malformed.
    pub fn process(&self, id: ProcessId) -> Result<Option<ProcessRecord>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let record = load(&transaction, id)?;
        transaction.commit()?;
        Ok(record)
    }

    /// Lists Processes in identifier order with the revision of the same read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn processes(
        &self,
        filter: ProcessFilter,
    ) -> Result<Committed<Vec<ProcessRecord>>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = crate::service_exposures::current_revision(&transaction)?;
        let mut statement = transaction.prepare(&format!(
            "SELECT {PROCESS_COLUMNS} FROM processes
             WHERE (?1 IS NULL OR creator_endpoint_id = ?1) AND (?2 IS NULL OR process_id > ?2)
             AND (?3 OR archived_at_ms IS NULL)
             ORDER BY process_id LIMIT ?4"
        ))?;
        let rows = statement.query_map(
            params![
                filter.creator.map(|creator| creator.as_bytes().to_vec()),
                filter.after.map(|after| after.as_bytes().to_vec()),
                filter.archived,
                filter.limit,
            ],
            read_row,
        )?;
        let records = rows
            .map(|row| decode_row(row?))
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(Committed::new(revision, records))
    }

    /// Returns every Process that has not reached a terminal state.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a row is malformed.
    pub fn active_processes(&self) -> Result<Vec<ProcessRecord>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {PROCESS_COLUMNS} FROM processes WHERE {ACTIVE} ORDER BY process_id"
        ))?;
        let rows = statement.query_map([], read_row)?;
        rows.map(|row| decode_row(row?)).collect()
    }
}
