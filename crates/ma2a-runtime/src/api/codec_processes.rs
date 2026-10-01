//! Local API commands for the Processes this Runtime hosts.

use ma2a_core::{
    RequestId,
    service::{MAX_TERMINATE_GRACE_MS, ProcessId},
};
use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_fields::{encode_hex, exact_fields, number, request_id, text},
    commands::CommandKind,
};

/// One local operator command on hosted Processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProcessCommand {
    /// Every Process, archived tombstones included, after this one.
    List(Option<ProcessId>),
    /// One Process in full.
    Inspect(ProcessId),
    /// End one Process with this grace period.
    Terminate(RequestId, ProcessId, u32),
    /// Archive one ended Process.
    Delete(RequestId, ProcessId),
}

impl ProcessCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::List(_) => "process_list",
            Self::Inspect(_) => "process_inspect",
            Self::Terminate(..) => "process_terminate",
            Self::Delete(..) => "process_delete",
        }
    }

    pub(crate) const fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::List(_) | Self::Inspect(_) => None,
            Self::Terminate(id, ..) | Self::Delete(id, _) => Some(*id),
        }
    }

    pub(super) fn to_value(&self) -> Value {
        let operation = self.operation();
        let hex = |id: &ProcessId| encode_hex(id.as_bytes());
        match self {
            Self::List(after) => json!({"operation": operation, "after": after.as_ref().map(hex)}),
            Self::Inspect(id) => json!({"operation": operation, "process_id": hex(id)}),
            Self::Terminate(request, id, grace_ms) => json!({
                "operation": operation,
                "request_id": encode_hex(request.as_bytes()),
                "process_id": hex(id),
                "grace_ms": grace_ms,
            }),
            Self::Delete(request, id) => json!({
                "operation": operation,
                "request_id": encode_hex(request.as_bytes()),
                "process_id": hex(id),
            }),
        }
    }
}

fn process_id(value: Option<&Value>) -> Result<ProcessId, ApiError> {
    value
        .and_then(Value::as_str)
        .and_then(|text| ProcessId::parse(text).ok())
        .ok_or_else(ApiError::invalid_input)
}

/// Parses one `process_*` command.
pub(super) fn parse(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    let command = match text(object, "operation")? {
        "process_list" => {
            exact_fields(object, &["version", "operation", "after"])?;
            let after = match object.get("after") {
                Some(Value::Null) => None,
                value => Some(process_id(value)?),
            };
            ProcessCommand::List(after)
        }
        "process_inspect" => {
            exact_fields(object, &["version", "operation", "process_id"])?;
            ProcessCommand::Inspect(process_id(object.get("process_id"))?)
        }
        "process_terminate" => {
            let fields = [
                "version",
                "operation",
                "request_id",
                "process_id",
                "grace_ms",
            ];
            exact_fields(object, &fields)?;
            let grace_ms = u32::try_from(number(object, "grace_ms")?)
                .ok()
                .filter(|grace_ms| *grace_ms <= MAX_TERMINATE_GRACE_MS)
                .ok_or_else(ApiError::invalid_input)?;
            ProcessCommand::Terminate(
                request_id(object, "request_id")?,
                process_id(object.get("process_id"))?,
                grace_ms,
            )
        }
        "process_delete" => {
            exact_fields(
                object,
                &["version", "operation", "request_id", "process_id"],
            )?;
            ProcessCommand::Delete(
                request_id(object, "request_id")?,
                process_id(object.get("process_id"))?,
            )
        }
        _ => return Err(ApiError::invalid_input()),
    };
    Ok(CommandKind::Process(command))
}
