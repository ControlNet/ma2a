//! JSON views of remote Process answers, shaped like the local API's `process_*` results.

use std::fmt::Write as _;

use ma2a_core::{
    RequestId,
    service::{
        ProcessAccept, ProcessDetails, ProcessReply, ProcessStart, ProcessSummary, Termination,
    },
};
use serde_json::{Value, json};

fn request_hex(id: &RequestId) -> String {
    id.as_bytes()
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _written = write!(hex, "{byte:02x}");
            hex
        })
}

pub(super) fn summary(summary: &ProcessSummary) -> Value {
    let (exit_code, signal) = match summary.termination {
        Some(Termination::Exited(code)) => (Some(code), None),
        Some(Termination::Signaled(signal)) => (None, Some(signal)),
        Some(Termination::Unknown) | None => (None, None),
    };
    json!({
        "process_id": summary.id.to_string(),
        "request_id": request_hex(&summary.request_id),
        "name": summary.name,
        "state": summary.state.name(),
        "reason": summary.reason.map(ma2a_core::service::ProcessReason::name),
        "exit_code": exit_code,
        "signal": signal,
        "created_at_ms": summary.times.0,
        "started_at_ms": summary.times.1,
        "ended_at_ms": summary.times.2,
    })
}

/// The start as its creator sent it; environment values are never printed, only names.
fn start(start: &ProcessStart) -> Value {
    let spec = start.spec();
    let labels: Vec<Value> = start
        .labels()
        .iter()
        .map(|(key, value)| json!({"key": key, "value": value}))
        .collect();
    json!({
        "program": spec.program(),
        "args": spec.args(),
        "cwd": spec.cwd(),
        "env_names": spec.env().iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "stdin": start.stdin(),
        "log_limit": start.log_limit(),
        "labels": labels,
    })
}

fn details(details: &ProcessDetails) -> Value {
    json!({
        "process": summary(&details.summary),
        "start": details.start.as_ref().map(start),
        "log_first_available": details.log.first_available,
        "log_end": details.log.end,
    })
}

/// One remote answer without a stream body.
pub(super) fn reply(accept: Option<ProcessAccept>, reply: &ProcessReply) -> Value {
    match reply {
        ProcessReply::Summary(process) => {
            let mut value = summary(process);
            if let (Some(ProcessAccept::Started(_, existing)), Some(object)) =
                (accept, value.as_object_mut())
            {
                object.insert("existing".to_owned(), json!(existing));
            }
            value
        }
        ProcessReply::Page(processes, next) => json!({
            "processes": processes.iter().map(summary).collect::<Vec<_>>(),
            "next": next.map(|id| id.to_string()),
        }),
        ProcessReply::Details(full) => details(full),
        ProcessReply::Logs(next, ended) => json!({"next": next, "ended": ended}),
        ProcessReply::Detached(reason, process, cursor) => json!({
            "reason": reason.name(),
            "process": summary(process),
            "cursor": cursor,
        }),
        ProcessReply::Written(bytes, closed) => json!({"written": bytes, "stdin_closed": closed}),
        ProcessReply::Deleted => json!({"deleted": true}),
    }
}
