//! Row encoding and decoding for durable Processes.

use ma2a_core::{
    EndpointId, RequestId,
    service::{ProcessId, ProcessReason, ProcessState, ProcessSummary, Termination},
};
use rusqlite::Row;

use crate::StoreError;

/// Every column of one Process row, in [`PROCESS_COLUMNS`] order.
pub(crate) const PROCESS_COLUMNS: &str = "process_id, creator_endpoint_id, request_id, fingerprint, \
     start, name, state, reason, termination_kind, termination_value, secret, created_at_ms, \
     started_at_ms, ended_at_ms, archived_at_ms";

const MALFORMED: StoreError = StoreError::SchemaMismatch {
    detail: "persisted Process is malformed",
};

/// Creation, start, end, and archive times of one Process in Unix milliseconds.
#[expect(
    clippy::exhaustive_structs,
    reason = "a Store row; views and tests name every field so none can be forgotten"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessTimes {
    /// When the Process was recorded.
    pub created_ms: u64,
    /// When its supervisor reported the child running.
    pub started_ms: Option<u64>,
    /// When it reached a terminal state.
    pub ended_ms: Option<u64>,
    /// When its output and specification were removed.
    pub archived_ms: Option<u64>,
}

/// One durable Process as the Store holds it.
#[expect(
    clippy::exhaustive_structs,
    reason = "a Store row; views and tests name every field so none can be forgotten"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRecord {
    /// Process identifier.
    pub id: ProcessId,
    /// The Endpoint that created it and alone may manage it remotely.
    pub creator: EndpointId,
    /// `RequestId` of the start that created it.
    pub request_id: RequestId,
    /// Fingerprint of the canonical start body.
    pub fingerprint: [u8; 32],
    /// Canonical start bytes, or `None` once archived.
    pub start: Option<Vec<u8>>,
    /// Display name, if any.
    pub name: Option<String>,
    /// Lifecycle state.
    pub state: ProcessState,
    /// Why it ended, for `terminated`, `failed`, and `lost`.
    pub reason: Option<ProcessReason>,
    /// How the child ended, when observed.
    pub termination: Option<Termination>,
    /// The supervisor ownership secret, held only while the Process is not terminal.
    pub secret: Option<[u8; 32]>,
    /// Lifecycle times.
    pub times: ProcessTimes,
}

impl ProcessRecord {
    /// Returns the content-free summary a creator sees.
    pub fn summary(&self) -> ProcessSummary {
        ProcessSummary::new(self.id, self.request_id, self.times.created_ms)
            .with_state(self.state, (self.reason, self.termination))
            .with_details(
                self.name.clone(),
                (self.times.started_ms, self.times.ended_ms),
            )
    }

    /// Returns whether the Process was archived.
    pub const fn is_archived(&self) -> bool {
        self.times.archived_ms.is_some()
    }
}

pub(crate) const fn state_code(state: ProcessState) -> u8 {
    match state {
        ProcessState::Created => 0,
        ProcessState::Starting => 1,
        ProcessState::Running => 2,
        ProcessState::Exited => 3,
        ProcessState::Terminated => 4,
        ProcessState::Failed => 5,
        ProcessState::Lost => 6,
    }
}

pub(crate) const fn reason_code(reason: ProcessReason) -> u8 {
    match reason {
        ProcessReason::SpawnFailed => 0,
        ProcessReason::Interrupted => 1,
        ProcessReason::Requested => 2,
        ProcessReason::Unauthorized => 3,
        ProcessReason::SupervisorLost => 4,
    }
}

pub(crate) const fn termination_columns(
    termination: Option<Termination>,
) -> (Option<u8>, Option<u32>) {
    match termination {
        None => (None, None),
        Some(Termination::Exited(code)) => (Some(0), Some(code)),
        Some(Termination::Signaled(signal)) => (Some(1), Some(signal)),
        Some(Termination::Unknown) => (Some(2), None),
    }
}

fn state(code: u8) -> Result<ProcessState, StoreError> {
    ProcessState::ALL
        .into_iter()
        .find(|state| state_code(*state) == code)
        .ok_or(MALFORMED)
}

fn reason(code: Option<u8>) -> Result<Option<ProcessReason>, StoreError> {
    code.map(|code| {
        ProcessReason::ALL
            .into_iter()
            .find(|reason| reason_code(*reason) == code)
            .ok_or(MALFORMED)
    })
    .transpose()
}

const fn termination(
    kind: Option<u8>,
    value: Option<u32>,
) -> Result<Option<Termination>, StoreError> {
    match (kind, value) {
        (None, None) => Ok(None),
        (Some(0), Some(code)) => Ok(Some(Termination::Exited(code))),
        (Some(1), Some(signal)) => Ok(Some(Termination::Signaled(signal))),
        (Some(2), None) => Ok(Some(Termination::Unknown)),
        _ => Err(MALFORMED),
    }
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], StoreError> {
    <[u8; N]>::try_from(bytes).map_err(|_| MALFORMED)
}

type RawRow = (
    (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>),
    (Option<Vec<u8>>, Option<String>, u8, Option<u8>),
    (Option<u8>, Option<u32>, Option<Vec<u8>>),
    (u64, Option<u64>, Option<u64>, Option<u64>),
);

/// Reads one row selected with [`PROCESS_COLUMNS`].
pub(crate) fn read_row(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?),
        (row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?),
        (row.get(8)?, row.get(9)?, row.get(10)?),
        (row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?),
    ))
}

/// Decodes one raw row, failing closed on any malformed column.
pub(crate) fn decode_row(raw: RawRow) -> Result<ProcessRecord, StoreError> {
    let (
        (id, creator, request_id, fingerprint),
        (start, name, state_value, reason_value),
        rest,
        times,
    ) = raw;
    let (kind, value, secret) = rest;
    Ok(ProcessRecord {
        id: ProcessId::from_bytes(fixed(&id)?),
        creator: EndpointId::try_from(creator.as_slice()).map_err(|_| MALFORMED)?,
        request_id: RequestId::try_from(request_id.as_slice()).map_err(|_| MALFORMED)?,
        fingerprint: fixed(&fingerprint)?,
        start,
        name,
        state: state(state_value)?,
        reason: reason(reason_value)?,
        termination: termination(kind, value)?,
        secret: secret.as_deref().map(fixed).transpose()?,
        times: ProcessTimes {
            created_ms: times.0,
            started_ms: times.1,
            ended_ms: times.2,
            archived_ms: times.3,
        },
    })
}
