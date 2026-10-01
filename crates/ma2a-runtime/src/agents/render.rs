//! Rendering durable deliveries as adapter lines.
//!
//! The outbox stores references only; every line is rebuilt from the task, its events,
//! and its tool calls, so a re-sent delivery is byte-for-byte the one first sent.

use ma2a_agent::protocol::{Origin, RuntimeToAgent, ToolError, encode_line};
use ma2a_core::service::AgentEventBody;
use ma2a_store::{AgentDelivery, AgentOutboxEntry, Repository, StoreError};
use serde::Deserialize;
use serde_json::Value;

use super::convert::{hex, to_wire};

/// A stored tool outcome: exactly one of its fields.
#[derive(Deserialize)]
struct StoredOutcome {
    #[serde(default)]
    ok: Option<Value>,
    #[serde(default)]
    error: Option<ToolError>,
}

/// One delivery's sequence number and its line, or nothing when its task or call is gone.
pub(super) type Rendered = (u64, Option<Vec<u8>>);

/// Renders every entry; one whose task or call is gone renders as nothing.
pub(super) fn lines(
    repository: &Repository,
    entries: &[AgentOutboxEntry],
) -> Result<Vec<Rendered>, StoreError> {
    entries
        .iter()
        .map(|entry| {
            let line =
                delivery(repository, entry)?.and_then(|delivery| encode_line(&delivery).ok());
            Ok((entry.seq, line))
        })
        .collect()
}

fn delivery(
    repository: &Repository,
    entry: &AgentOutboxEntry,
) -> Result<Option<RuntimeToAgent>, StoreError> {
    let seq = entry.seq;
    Ok(match &entry.delivery {
        AgentDelivery::Task(id) => repository.agent_task(*id)?.map(|record| {
            let summary = &record.summary;
            RuntimeToAgent::Task {
                seq,
                task: id.to_string(),
                from: Origin {
                    endpoint: hex(summary.creator.as_bytes()),
                    agent: summary.from_agent.as_ref().map(ToString::to_string),
                },
                message: to_wire(&record.input),
            }
        }),
        AgentDelivery::Message(id, event) => repository
            .agent_task_events(*id, (event.saturating_sub(1), 1))?
            .into_iter()
            .find(|found| found.seq == *event)
            .and_then(|found| match found.body {
                AgentEventBody::Message(message) => Some(RuntimeToAgent::Message {
                    seq,
                    task: id.to_string(),
                    message: to_wire(&message),
                }),
                _ => None,
            }),
        AgentDelivery::Cancel(id) => Some(RuntimeToAgent::Cancel {
            seq,
            task: id.to_string(),
        }),
        AgentDelivery::ToolResult(id, call) => repository
            .tool_call((*id, call.as_str()))?
            .and_then(|recorded| recorded.result)
            .and_then(|result| serde_json::from_slice::<StoredOutcome>(&result).ok())
            .map(|outcome| RuntimeToAgent::ToolResult {
                seq,
                task: id.to_string(),
                call: call.clone(),
                ok: outcome.ok,
                error: outcome.error,
            }),
    })
}
