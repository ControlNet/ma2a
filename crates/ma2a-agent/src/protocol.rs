//! The adapter protocol: one JSON object per line, tagged by `"t"`.
//!
//! The Agent writes [`AgentToRuntime`] lines to its stdout and reads [`RuntimeToAgent`]
//! lines from its stdin. Every Runtime line carries an instance-scoped `seq`: the Runtime
//! re-sends every delivery the Agent has not acknowledged, so the Agent processes each
//! `seq` once and acknowledges in order.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The adapter protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// Longest Agent line in bytes, without its newline; a longer one is ignored.
pub const MAX_AGENT_LINE_LEN: usize = 65_536;
/// Longest Runtime line in bytes, without its newline.
pub const MAX_RUNTIME_LINE_LEN: usize = 16 * 1_048_576;
/// Longest tool name in bytes; a call naming a longer (or empty) one is ignored.
pub const MAX_TOOL_NAME_LEN: usize = 64;
/// Longest tool `args` in bytes, as compact JSON; a call with longer ones is ignored.
pub const MAX_TOOL_ARGS_LEN: usize = 16_384;
/// Longest tool answer in bytes, as the compact JSON `{"ok": …}` or `{"error": …}`; a
/// longer answer is replaced by a `too_large` error.
pub const MAX_TOOL_RESULT_LEN: usize = 65_536;

/// Who wrote a message.
#[expect(
    clippy::exhaustive_enums,
    reason = "message roles are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The task's creator.
    User,
    /// The Agent.
    Agent,
}

/// One part of a message.
#[expect(
    clippy::exhaustive_enums,
    reason = "message parts are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Part {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// One JSON value.
    Data {
        /// The value.
        data: Value,
    },
}

/// One message of a task's conversation.
#[expect(
    clippy::exhaustive_structs,
    reason = "a message is exactly its role and parts"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Who wrote it.
    pub role: Role,
    /// Its parts, in order.
    pub parts: Vec<Part>,
}

impl Message {
    /// Returns an Agent message of one text part.
    pub fn agent_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Agent,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    /// Returns a user message of one text part.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    /// Returns every text part joined by newlines.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text { text } => Some(text.as_str()),
                Part::Data { .. } => None,
            })
            .collect();
        texts.join("\n")
    }
}

/// A state the Agent reports for one of its tasks.
#[expect(
    clippy::exhaustive_enums,
    reason = "reportable states are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    /// Working on it.
    Running,
    /// Waiting for the creator's next message.
    Waiting,
    /// Done; the result is the report's message and the task's artifacts.
    Completed,
    /// Could not do it; the report's error says why.
    Failed,
    /// Stopped after the Runtime delivered `cancel`.
    Cancelled,
}

/// Who created a delivered task.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// The creating Endpoint, as its canonical text.
    pub endpoint: String,
    /// The Agent the creator says asked; attribution only, never authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Why a tool call did not succeed.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolError {
    /// A stable status name, such as `tool_denied` or a Services v1 status.
    pub code: String,
    /// A human-readable explanation.
    pub message: String,
}

impl ToolError {
    /// Creates one error.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

/// One line the Agent writes to stdout.
#[expect(
    clippy::exhaustive_enums,
    reason = "the adapter protocol is a closed, versioned vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentToRuntime {
    /// The Agent is ready for tasks; the first line, within 30 seconds of starting.
    Ready {
        /// [`PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// Every delivery up to and including `seq` was processed.
    Ack {
        /// The newest processed delivery.
        seq: u64,
    },
    /// A task's new state.
    State {
        /// The task.
        task: String,
        /// Its new state.
        state: ReportedState,
        /// The inline result of `completed`, or the question of `waiting`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Message>,
        /// Why a task `failed`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// One Agent message of a task's conversation.
    Message {
        /// The task.
        task: String,
        /// The message.
        message: Message,
    },
    /// Transient progress; only the newest progress of a task is retained.
    Progress {
        /// The task.
        task: String,
        /// At most 1 KiB of text.
        text: String,
    },
    /// One tool call, answered by a `tool_result` with the same `call`.
    Tool {
        /// The task the call works for.
        task: String,
        /// Unique per task: 1–64 bytes of `[A-Za-z0-9._-]`.
        call: String,
        /// The tool, such as `files.stat`.
        name: String,
        /// The tool's arguments object.
        args: Value,
    },
}

/// One line the Runtime writes to the Agent's stdin.
#[expect(
    clippy::exhaustive_enums,
    reason = "the adapter protocol is a closed, versioned vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeToAgent {
    /// A new task for this Agent.
    Task {
        /// Delivery sequence number.
        seq: u64,
        /// The task.
        task: String,
        /// Who created it.
        from: Origin,
        /// The submitted message.
        message: Message,
    },
    /// The creator's next message for a task.
    Message {
        /// Delivery sequence number.
        seq: u64,
        /// The task.
        task: String,
        /// The message.
        message: Message,
    },
    /// The creator asked to cancel a task.
    Cancel {
        /// Delivery sequence number.
        seq: u64,
        /// The task.
        task: String,
    },
    /// The answer to one tool call: exactly one of `ok` and `error`.
    ToolResult {
        /// Delivery sequence number.
        seq: u64,
        /// The task.
        task: String,
        /// The call it answers.
        call: String,
        /// The tool's result object.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<Value>,
        /// Why the call did not succeed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ToolError>,
    },
}

impl RuntimeToAgent {
    /// Returns the delivery's sequence number.
    pub const fn seq(&self) -> u64 {
        match self {
            Self::Task { seq, .. }
            | Self::Message { seq, .. }
            | Self::Cancel { seq, .. }
            | Self::ToolResult { seq, .. } => *seq,
        }
    }

    /// Returns the task the delivery is for.
    pub fn task(&self) -> &str {
        match self {
            Self::Task { task, .. }
            | Self::Message { task, .. }
            | Self::Cancel { task, .. }
            | Self::ToolResult { task, .. } => task,
        }
    }
}

/// Encodes one value as a line: compact JSON and a newline.
///
/// # Errors
/// Returns the serialization failure of a value that is not representable as JSON.
pub fn encode_line<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

/// Decodes one line without its newline; a trailing carriage return is accepted.
///
/// # Errors
/// Returns the parse failure of a line that is not one valid message.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &[u8]) -> Result<T, serde_json::Error> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    serde_json::from_slice(line)
}

/// Returns whether `call` is a valid tool call identifier.
pub fn is_call_id(call: &str) -> bool {
    (1..=64).contains(&call.len())
        && call
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
