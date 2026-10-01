//! Local API views of hosted Processes. Environment values are never shown, only names.

use ma2a_core::service::{LogWindow, ProcessStart, Termination};
use ma2a_store::ProcessRecord;
use serde_json::{Value, json};

use super::{
    codec_fields::encode_hex,
    responses::{CommandResult, ResultKind},
};

/// Most Processes one `process_list` page carries.
pub const MAX_PROCESS_LIST_PAGE: usize = 64;
/// Encoded start size above which its arguments are elided to keep the response bounded.
const MAX_START_VIEW_BYTES: usize = 32_768;

/// One Process as the local operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessView(ProcessRecord);

impl ProcessView {
    /// Wraps one durable Process record.
    pub const fn new(record: ProcessRecord) -> Self {
        Self(record)
    }

    fn to_value(&self) -> Value {
        let record = &self.0;
        let (exit_code, signal) = match record.termination {
            Some(Termination::Exited(code)) => (Some(code), None),
            Some(Termination::Signaled(signal)) => (None, Some(signal)),
            Some(Termination::Unknown) | None => (None, None),
        };
        json!({
            "process_id": encode_hex(record.id.as_bytes()),
            "creator_endpoint_id": encode_hex(record.creator.as_bytes()),
            "request_id": encode_hex(record.request_id.as_bytes()),
            "name": record.name,
            "state": record.state.name(),
            "reason": record.reason.map(ma2a_core::service::ProcessReason::name),
            "exit_code": exit_code,
            "signal": signal,
            "created_at_ms": record.times.created_ms,
            "started_at_ms": record.times.started_ms,
            "ended_at_ms": record.times.ended_ms,
            "archived": record.is_archived(),
        })
    }
}

fn start_value(start: &ProcessStart) -> Value {
    let spec = start.spec();
    let labels: Vec<Value> = start
        .labels()
        .iter()
        .map(|(key, value)| json!({"key": key, "value": value}))
        .collect();
    let mut value = json!({
        "program": spec.program(),
        "args": spec.args(),
        "args_elided": false,
        "cwd": spec.cwd(),
        "env_names": spec.env().iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "stdin": start.stdin(),
        "log_limit": start.log_limit(),
        "labels": labels,
    });
    let encoded = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
    if encoded > MAX_START_VIEW_BYTES
        && let Some(object) = value.as_object_mut()
    {
        object.insert("args".to_owned(), json!([]));
        object.insert("args_elided".to_owned(), json!(true));
    }
    value
}

/// One Process in full: its start, while retained, and its output window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessDetailsView {
    process: ProcessView,
    start: Option<ProcessStart>,
    log: LogWindow,
}

impl ProcessDetailsView {
    /// Pairs a Process with its start, while retained, and its output window.
    pub fn new(process: ProcessView, (start, log): (Option<ProcessStart>, LogWindow)) -> Self {
        Self {
            process,
            start,
            log,
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "process": self.process.to_value(),
            "start": self.start.as_ref().map(start_value),
            "log_first_available": self.log.first_available,
            "log_end": self.log.end,
        })
    }
}

/// The result of one `process_*` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProcessResult {
    Page(Vec<ProcessView>, Option<ma2a_core::service::ProcessId>),
    Details(Box<ProcessDetailsView>),
    Terminated(ProcessView),
    Deleted(ProcessView),
}

impl ProcessResult {
    pub(super) const fn result_type(&self) -> &'static str {
        match self {
            Self::Page(..) => "processes",
            Self::Details(_) => "process",
            Self::Terminated(_) => "process_terminated",
            Self::Deleted(_) => "process_deleted",
        }
    }

    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Page(processes, next) => json!({
                "processes": processes.iter().map(ProcessView::to_value).collect::<Vec<_>>(),
                "next": next.map(|id| encode_hex(id.as_bytes())),
            }),
            Self::Details(details) => details.to_value(),
            Self::Terminated(process) | Self::Deleted(process) => process.to_value(),
        }
    }
}

impl CommandResult {
    /// Creates one bounded `processes` page; `next` continues it.
    ///
    /// # Errors
    /// Returns `invalid_input` above [`MAX_PROCESS_LIST_PAGE`] Processes.
    pub fn processes(
        processes: Vec<ProcessView>,
        next: Option<ma2a_core::service::ProcessId>,
    ) -> Result<Self, super::ApiError> {
        if processes.len() > MAX_PROCESS_LIST_PAGE {
            return Err(super::ApiError::invalid_input());
        }
        Ok(Self::process(ProcessResult::Page(processes, next)))
    }

    /// Creates a `process` result.
    pub fn process_details(details: ProcessDetailsView) -> Self {
        Self::process(ProcessResult::Details(Box::new(details)))
    }

    /// Creates a `process_terminated` result.
    pub const fn process_terminated(process: ProcessView) -> Self {
        Self::process(ProcessResult::Terminated(process))
    }

    /// Creates a `process_deleted` result.
    pub const fn process_deleted(process: ProcessView) -> Self {
        Self::process(ProcessResult::Deleted(process))
    }

    const fn process(result: ProcessResult) -> Self {
        Self(ResultKind::Process(result), None)
    }
}
