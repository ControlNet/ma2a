//! The arguments and results of every tool, as JSON records.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::protocol::Message;

/// Longest `agent.wait` in milliseconds.
pub const MAX_AGENT_WAIT_MS: u64 = 300_000;
/// Most bytes of stdout, and of stderr, an `exec.run` result carries.
pub const MAX_EXEC_CAPTURE: usize = 1_048_576;

/// `files.stat`, `files.list`: one path in one exported root.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesTarget {
    /// Target Endpoint.
    pub endpoint: String,
    /// Exported root name.
    pub root: String,
    /// Absolute path inside the root, such as `/reports/a.txt`.
    pub path: String,
    /// `files.list` only: the page cursor a previous listing returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// What `files.stat` reports.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileInfo {
    /// `file`, `directory`, `symlink`, or `other`.
    pub kind: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in Unix milliseconds, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
    /// Whether the target user cannot write it.
    pub read_only: bool,
}

/// One directory entry.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedEntry {
    /// Entry name.
    pub name: String,
    /// `file`, `directory`, `symlink`, or `other`.
    pub kind: String,
    /// Size in bytes.
    pub size: u64,
}

/// One `files.list` page.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    /// Entries in name order.
    pub entries: Vec<ListedEntry>,
    /// Cursor of the next page, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// `transfer.get`: download one remote file into the workspace.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferGet {
    /// Target Endpoint.
    pub endpoint: String,
    /// Exported root name.
    pub root: String,
    /// Absolute path inside the root.
    pub path: String,
    /// Workspace-relative destination; replaced when it exists.
    pub to: String,
}

/// `transfer.put`: upload one workspace file.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferPut {
    /// Target Endpoint.
    pub endpoint: String,
    /// Exported root name.
    pub root: String,
    /// Absolute destination path inside the root.
    pub path: String,
    /// Workspace-relative source.
    pub from: String,
    /// Whether an existing destination is replaced.
    #[serde(default)]
    pub replace: bool,
}

/// What a verified transfer moved.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transferred {
    /// Bytes moved.
    pub size: u64,
    /// BLAKE3 of the content, as lowercase hex.
    pub blake3: String,
}

/// `exec.run`: one program on a remote Endpoint, stdin empty.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRun {
    /// Target Endpoint.
    pub endpoint: String,
    /// Program to run.
    pub program: String,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Absolute working directory on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Kill after this long; at most 24 hours, default 60 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// What `exec.run` observed.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecOutput {
    /// Exit code, if the program exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    /// Signal number, if a signal ended it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u32>,
    /// `completed`, `timed_out`, or another end reason.
    pub reason: String,
    /// Stdout as lossy UTF-8, at most [`MAX_EXEC_CAPTURE`] bytes.
    pub stdout: String,
    /// Stderr as lossy UTF-8, at most [`MAX_EXEC_CAPTURE`] bytes.
    pub stderr: String,
    /// Whether either stream produced more than it carries.
    pub truncated: bool,
}

/// `process.start`: one durable Process on a remote Endpoint.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessStartArgs {
    /// Target Endpoint.
    pub endpoint: String,
    /// Program to run.
    pub program: String,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Absolute working directory on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `process.inspect`: one Process this Endpoint created.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessTarget {
    /// Target Endpoint.
    pub endpoint: String,
    /// Process identifier, as 32 hex digits.
    pub id: String,
}

/// `process.terminate`: end one Process, killing it after the grace period.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessTerminate {
    /// Target Endpoint.
    pub endpoint: String,
    /// Process identifier.
    pub id: String,
    /// Grace period; at most 60 seconds, default 5 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_ms: Option<u32>,
}

/// One Process as its creator sees it.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessInfo {
    /// Process identifier.
    pub id: String,
    /// Lifecycle state name.
    pub state: String,
    /// Why it ended, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Exit code, once observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
}

/// `proxy.open`: a loopback listener relaying to `host:port` as seen by the target.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyOpen {
    /// Target Endpoint.
    pub endpoint: String,
    /// Destination host.
    pub host: String,
    /// Destination port.
    pub port: u16,
}

/// Where a `proxy.open` listener accepts connections, until the task ends.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyListening {
    /// `127.0.0.1:PORT`.
    pub listen: String,
}

/// `agent.submit`: a task on another Endpoint's Agent.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSubmitArgs {
    /// Target Endpoint.
    pub endpoint: String,
    /// Target Agent.
    pub agent: String,
    /// The task's first message, with role `user`.
    pub message: Message,
}

/// `agent.get`, `agent.cancel`: one task this Endpoint created.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTaskTarget {
    /// Target Endpoint.
    pub endpoint: String,
    /// Target Agent.
    pub agent: String,
    /// Task identifier, as 32 hex digits.
    pub task: String,
}

/// `agent.wait`: one task's view once it ended, or when the wait timed out.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWait {
    /// Target Endpoint.
    pub endpoint: String,
    /// Target Agent.
    pub agent: String,
    /// Task identifier.
    pub task: String,
    /// Longest wait; at most [`MAX_AGENT_WAIT_MS`], which is also the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `agent.send`: the next message of a task waiting for input.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSendArgs {
    /// Target Endpoint.
    pub endpoint: String,
    /// Target Agent.
    pub agent: String,
    /// Task identifier.
    pub task: String,
    /// The message, with role `user`.
    pub message: Message,
}

/// `agent.artifact`: download one artifact of a task into the workspace.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentArtifactArgs {
    /// Target Endpoint.
    pub endpoint: String,
    /// Target Agent.
    pub agent: String,
    /// Task identifier.
    pub task: String,
    /// Artifact name.
    pub name: String,
    /// Workspace-relative destination; replaced when it exists.
    pub to: String,
}

/// One artifact reference.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInfo {
    /// Task-local name.
    pub name: String,
    /// Media type.
    pub media_type: String,
    /// Size in bytes.
    pub size: u64,
    /// BLAKE3 of the content, as lowercase hex.
    pub blake3: String,
}

/// One task as its creator sees it.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskView {
    /// Task identifier.
    pub task: String,
    /// `queued`, `running`, `waiting`, `completed`, `failed`, or `cancelled`.
    pub state: String,
    /// Why it ended, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The inline result of a completed task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Message>,
    /// The failure the Agent reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The newest progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Published artifacts.
    #[serde(default)]
    pub artifacts: Vec<ArtifactInfo>,
}

/// `artifact.publish`: one workspace file becomes an artifact of the calling task.
#[expect(clippy::exhaustive_structs, reason = "a closed wire record")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPublish {
    /// Task-local name: 1–128 bytes of `[A-Za-z0-9._-]`, not starting with a dot.
    pub name: String,
    /// Workspace-relative source.
    pub path: String,
    /// Media type; default `application/octet-stream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// The published artifact.
pub type Published = ArtifactInfo;
