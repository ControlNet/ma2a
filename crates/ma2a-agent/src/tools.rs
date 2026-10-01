//! The tools a Runtime offers its Agents: names, families, arguments, and results.
//!
//! Every tool except `artifact.publish` targets a remote Endpoint and runs with exactly
//! the hosting Endpoint's authority. Workspace paths (`to`, `from`, `path`) are relative to
//! the instance workspace and never leave it.

mod records;

pub use records::{
    AgentArtifactArgs, AgentSendArgs, AgentSubmitArgs, AgentTaskTarget, AgentWait, ArtifactInfo,
    ArtifactPublish, ExecOutput, ExecRun, FileInfo, FilesTarget, ListedEntry, Listing,
    MAX_AGENT_WAIT_MS, MAX_EXEC_CAPTURE, ProcessInfo, ProcessStartArgs, ProcessTarget,
    ProcessTerminate, ProxyListening, ProxyOpen, Published, TaskView, TransferGet, TransferPut,
    Transferred,
};

/// A group of tools an Agent definition allows as a whole.
#[expect(
    clippy::exhaustive_enums,
    reason = "tool families are a closed, versioned vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolFamily {
    /// `files.stat`, `files.list`.
    Files,
    /// `transfer.get`, `transfer.put`.
    Transfer,
    /// `exec.run`.
    Exec,
    /// `process.start`, `process.inspect`, `process.terminate`.
    Process,
    /// `proxy.open`.
    Proxy,
    /// `agent.*`: tasks on other Endpoints' Agents.
    Agent,
}

impl ToolFamily {
    /// Every family, in bit order.
    pub const ALL: [Self; 6] = [
        Self::Files,
        Self::Transfer,
        Self::Exec,
        Self::Process,
        Self::Proxy,
        Self::Agent,
    ];

    /// Returns the family's bit in a definition's tool set.
    pub const fn bit(self) -> u8 {
        match self {
            Self::Files => 1,
            Self::Transfer => 2,
            Self::Exec => 4,
            Self::Process => 8,
            Self::Proxy => 16,
            Self::Agent => 32,
        }
    }

    /// Returns the family's stable name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Transfer => "transfer",
            Self::Exec => "exec",
            Self::Process => "process",
            Self::Proxy => "proxy",
            Self::Agent => "agent",
        }
    }

    /// Parses a stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|family| family.name() == name)
    }

    /// Returns the families of a tool set, in bit order.
    pub fn of_set(set: u8) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|family| set & family.bit() != 0)
            .collect()
    }
}

/// One tool.
#[expect(
    clippy::exhaustive_enums,
    reason = "tools are a closed, versioned vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// [`FilesTarget`] → [`FileInfo`].
    FilesStat,
    /// [`FilesTarget`] → [`Listing`].
    FilesList,
    /// [`TransferGet`] → [`Transferred`].
    TransferGet,
    /// [`TransferPut`] → [`Transferred`].
    TransferPut,
    /// [`ExecRun`] → [`ExecOutput`].
    ExecRun,
    /// [`ProcessStartArgs`] → [`ProcessInfo`].
    ProcessStart,
    /// [`ProcessTarget`] → [`ProcessInfo`].
    ProcessInspect,
    /// [`ProcessTerminate`] → [`ProcessInfo`].
    ProcessTerminate,
    /// [`ProxyOpen`] → [`ProxyListening`].
    ProxyOpen,
    /// [`AgentSubmitArgs`] → [`TaskView`].
    AgentSubmit,
    /// [`AgentTaskTarget`] → [`TaskView`].
    AgentGet,
    /// [`AgentWait`] → [`TaskView`], once the task ended or the wait timed out.
    AgentWait,
    /// [`AgentSendArgs`] → [`TaskView`].
    AgentSend,
    /// [`AgentTaskTarget`] → [`TaskView`], once the task ended.
    AgentCancel,
    /// [`AgentArtifactArgs`] → [`Transferred`].
    AgentArtifact,
    /// [`ArtifactPublish`] → [`Published`]; always allowed.
    ArtifactPublish,
}

impl Tool {
    /// Every tool.
    pub const ALL: [Self; 16] = [
        Self::FilesStat,
        Self::FilesList,
        Self::TransferGet,
        Self::TransferPut,
        Self::ExecRun,
        Self::ProcessStart,
        Self::ProcessInspect,
        Self::ProcessTerminate,
        Self::ProxyOpen,
        Self::AgentSubmit,
        Self::AgentGet,
        Self::AgentWait,
        Self::AgentSend,
        Self::AgentCancel,
        Self::AgentArtifact,
        Self::ArtifactPublish,
    ];

    /// Returns the tool's stable name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::FilesStat => "files.stat",
            Self::FilesList => "files.list",
            Self::TransferGet => "transfer.get",
            Self::TransferPut => "transfer.put",
            Self::ExecRun => "exec.run",
            Self::ProcessStart => "process.start",
            Self::ProcessInspect => "process.inspect",
            Self::ProcessTerminate => "process.terminate",
            Self::ProxyOpen => "proxy.open",
            Self::AgentSubmit => "agent.submit",
            Self::AgentGet => "agent.get",
            Self::AgentWait => "agent.wait",
            Self::AgentSend => "agent.send",
            Self::AgentCancel => "agent.cancel",
            Self::AgentArtifact => "agent.artifact",
            Self::ArtifactPublish => "artifact.publish",
        }
    }

    /// Parses a stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// Returns the family that must be allowed; `None` for a tool every Agent has.
    pub const fn family(self) -> Option<ToolFamily> {
        match self {
            Self::FilesStat | Self::FilesList => Some(ToolFamily::Files),
            Self::TransferGet | Self::TransferPut => Some(ToolFamily::Transfer),
            Self::ExecRun => Some(ToolFamily::Exec),
            Self::ProcessStart | Self::ProcessInspect | Self::ProcessTerminate => {
                Some(ToolFamily::Process)
            }
            Self::ProxyOpen => Some(ToolFamily::Proxy),
            Self::AgentSubmit
            | Self::AgentGet
            | Self::AgentWait
            | Self::AgentSend
            | Self::AgentCancel
            | Self::AgentArtifact => Some(ToolFamily::Agent),
            Self::ArtifactPublish => None,
        }
    }
}
