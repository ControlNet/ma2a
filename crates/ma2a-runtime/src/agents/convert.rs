//! Conversions between durable Agent types and the adapter's JSON shapes.

use ma2a_agent::{
    protocol::{Message, Part, Role},
    tools::{ArtifactInfo, TaskView},
};
use ma2a_core::service::{
    AgentMessage, AgentTaskDetails, ArtifactRef, MessagePart, MessageRole, ServiceError,
};
use serde_json::Value;

/// Returns the lowercase hex of one hash.
pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _written = write!(text, "{byte:02x}");
    }
    text
}

/// Renders one durable message; a `data` part is JSON text that was validated on entry.
pub(super) fn to_wire(message: &AgentMessage) -> Message {
    let role = match message.role() {
        MessageRole::User => Role::User,
        MessageRole::Agent => Role::Agent,
    };
    let parts = message
        .parts()
        .iter()
        .map(|part| match part {
            MessagePart::Text(text) => Part::Text { text: text.clone() },
            MessagePart::Data(json) => Part::Data {
                data: serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.clone())),
            },
        })
        .collect();
    Message { role, parts }
}

/// Validates one adapter or API message into its durable form.
///
/// # Errors
/// Returns [`ServiceError::InvalidInput`] when it exceeds a message bound.
pub(super) fn from_wire(message: &Message) -> Result<AgentMessage, ServiceError> {
    let role = match message.role {
        Role::User => MessageRole::User,
        Role::Agent => MessageRole::Agent,
    };
    let parts = message
        .parts
        .iter()
        .map(|part| match part {
            Part::Text { text } => MessagePart::Text(text.clone()),
            Part::Data { data } => MessagePart::Data(data.to_string()),
        })
        .collect();
    AgentMessage::new(role, parts).map_err(|_| ServiceError::InvalidInput)
}

/// Returns whether every `data` part of a message is one JSON value.
pub(super) fn valid_data(message: &AgentMessage) -> bool {
    message.parts().iter().all(|part| match part {
        MessagePart::Data(json) => serde_json::from_str::<Value>(json).is_ok(),
        MessagePart::Text(_) => true,
    })
}

/// Renders one artifact reference.
pub(super) fn artifact(artifact: &ArtifactRef) -> ArtifactInfo {
    ArtifactInfo {
        name: artifact.name().to_owned(),
        media_type: artifact.media_type().to_owned(),
        size: artifact.size(),
        blake3: hex(artifact.blake3()),
    }
}

/// Renders one task as its creator sees it.
pub(super) fn task_view(details: &AgentTaskDetails) -> TaskView {
    let summary = &details.summary;
    TaskView {
        task: summary.id.to_string(),
        state: summary.state.name().to_owned(),
        reason: summary.reason.map(|reason| reason.name().to_owned()),
        result: details.result.as_ref().map(to_wire),
        error: details.error.clone(),
        progress: details.progress.clone(),
        artifacts: details.artifacts.iter().map(artifact).collect(),
    }
}
