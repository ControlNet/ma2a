//! Local API views of hosted Agents and their tasks. Environment values are never shown,
//! only names, and every view is bounded so one page always fits one response.

use ma2a_agent::tools::ToolFamily;
use ma2a_core::service::{
    AgentEvent, AgentEventBody, AgentMessage, AgentTaskDetails, AgentTaskSummary, ArtifactRef,
    ProcessStart,
};
use ma2a_store::{AgentInstanceRecord, AgentRecord};
use serde_json::{Value, json};

use super::{codec_fields::encode_hex, codec_messages::message_value};

/// Encoded size above which one Agent view drops its arguments and environment names.
const MAX_COMMAND_VIEW_BYTES: usize = 2_048;
/// Encoded size above which a message is elided from a view.
const MAX_MESSAGE_VIEW_BYTES: usize = 20_480;

pub(super) fn encoded_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// One defined Agent as the local operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentView {
    record: AgentRecord,
    live_tasks: usize,
}

impl AgentView {
    /// Pairs one durable Agent record with its number of unfinished tasks.
    pub const fn new(record: AgentRecord, live_tasks: usize) -> Self {
        Self { record, live_tasks }
    }

    pub(super) fn id(&self) -> &str {
        self.record.definition.id.as_str()
    }

    pub(super) fn to_value(&self) -> Value {
        let definition = &self.record.definition;
        let start = ProcessStart::from_bytes(&definition.start).ok();
        let spec = start.as_ref().map(ProcessStart::spec);
        let mut args = json!(spec.map_or(&[][..], |spec| spec.args()));
        let names: Vec<&str> = spec.map_or_else(Vec::new, |spec| {
            spec.env().iter().map(|(name, _)| name.as_str()).collect()
        });
        let mut env_names = json!(names);
        let elided =
            encoded_len(&args).saturating_add(encoded_len(&env_names)) > MAX_COMMAND_VIEW_BYTES;
        if elided {
            (args, env_names) = (json!([]), json!([]));
        }
        let tools: Vec<&str> = ToolFamily::of_set(definition.tools)
            .into_iter()
            .map(ToolFamily::name)
            .collect();
        json!({
            "agent": definition.id.as_str(),
            "description": definition.description,
            "program": spec.and_then(|spec| spec.program()),
            "args": args,
            "env_names": env_names,
            "elided": elided,
            "cwd": spec.and_then(|spec| spec.cwd()),
            "tools": tools,
            "max_tasks": definition.max_tasks,
            "enabled": self.record.enabled,
            "generation": self.record.generation,
            "defined_at_ms": self.record.times.0,
            "changed_at_ms": self.record.times.1,
            "live_tasks": self.live_tasks,
            "instance": self.record.instance.as_ref().map(instance_value),
        })
    }
}

fn instance_value(instance: &AgentInstanceRecord) -> Value {
    json!({
        "generation": instance.generation,
        "process_id": instance.process_id.map(|id| encode_hex(id.as_bytes())),
        "ready": instance.ready,
        "started_at_ms": instance.started_ms,
        "ended_at_ms": instance.ended_ms,
    })
}

/// The local API view of one task's summary.
pub fn summary_value(summary: &AgentTaskSummary) -> Value {
    let (created, started, ended) = summary.times;
    json!({
        "task_id": summary.id.to_string(),
        "agent": summary.agent.as_str(),
        "creator_endpoint_id": encode_hex(summary.creator.as_bytes()),
        "from_agent": summary.from_agent.as_ref().map(ma2a_core::service::AgentId::as_str),
        "request_id": encode_hex(summary.request_id.as_bytes()),
        "state": summary.state.name(),
        "reason": summary.reason.map(ma2a_core::service::AgentTaskReason::name),
        "created_at_ms": created,
        "started_at_ms": started,
        "ended_at_ms": ended,
        "last_seq": summary.last_seq,
    })
}

/// A message and whether it was elided to keep the view bounded.
fn bounded_message(message: Option<&AgentMessage>) -> (Value, bool) {
    match message.map(message_value) {
        Some(value) if encoded_len(&value) > MAX_MESSAGE_VIEW_BYTES => (Value::Null, true),
        Some(value) => (value, false),
        None => (Value::Null, false),
    }
}

fn artifact_value(artifact: &ArtifactRef) -> Value {
    json!({
        "name": artifact.name(),
        "media_type": artifact.media_type(),
        "size": artifact.size(),
        "blake3": encode_hex(artifact.blake3()),
    })
}

/// The local API view of one task in full; an oversized message is elided and flagged.
pub fn details_value(details: &AgentTaskDetails) -> Value {
    let (input, input_elided) = bounded_message(Some(&details.input));
    let (result, result_elided) = bounded_message(details.result.as_ref());
    json!({
        "task": summary_value(&details.summary),
        "input": input,
        "input_elided": input_elided,
        "result": result,
        "result_elided": result_elided,
        "error": details.error,
        "progress": details.progress,
        "artifacts": details.artifacts.iter().map(artifact_value).collect::<Vec<_>>(),
    })
}

/// The local API view of one task event.
pub fn event_value(event: &AgentEvent) -> Value {
    let (mut state, mut reason, mut text, mut artifact) = (None, None, None, None);
    let (mut message, mut message_elided) = (Value::Null, false);
    let kind = match &event.body {
        AgentEventBody::State(next, why) => {
            state = Some(next.name());
            reason = why.map(ma2a_core::service::AgentTaskReason::name);
            "state"
        }
        AgentEventBody::Message(body) => {
            (message, message_elided) = bounded_message(Some(body));
            "message"
        }
        AgentEventBody::Progress(progress) => {
            text = Some(progress.as_str());
            "progress"
        }
        AgentEventBody::Artifact(published) => {
            artifact = Some(artifact_value(published));
            "artifact"
        }
    };
    json!({
        "seq": event.seq,
        "at_ms": event.at_ms,
        "kind": kind,
        "state": state,
        "reason": reason,
        "message": message,
        "message_elided": message_elided,
        "text": text,
        "artifact": artifact,
    })
}
