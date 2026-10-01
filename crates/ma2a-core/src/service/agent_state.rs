//! Agent task lifecycle states and end reasons.

use crate::ProtocolError;

/// The lifecycle state of one Agent task.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent task states are a closed, durable vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentTaskState {
    /// Recorded with its `RequestId` binding; not yet assigned to an instance.
    Queued,
    /// Assigned to one instance generation and delivered to it.
    Running,
    /// The Agent asked for more input.
    Waiting,
    /// The Agent reported a result.
    Completed,
    /// The Agent reported a failure, or could not finish the task.
    Failed,
    /// Cancelled on request or on withdrawn authorization.
    Cancelled,
}

impl AgentTaskState {
    /// Every state in wire-code order.
    pub const ALL: [Self; 6] = [
        Self::Queued,
        Self::Running,
        Self::Waiting,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns the state with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }

    /// Returns whether no further transition is possible.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns whether the lifecycle permits moving from `self` to `next`.
    pub const fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Running | Self::Failed | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Waiting | Self::Completed | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Waiting,
                    Self::Running | Self::Completed | Self::Failed | Self::Cancelled
                )
        )
    }

    /// Returns the stable durable code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Queued => 0,
            Self::Running => 1,
            Self::Waiting => 2,
            Self::Completed => 3,
            Self::Failed => 4,
            Self::Cancelled => 5,
        }
    }

    /// Returns the state with this durable code.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an unknown code.
    pub fn from_code(code: u64) -> Result<Self, ProtocolError> {
        Self::ALL
            .into_iter()
            .find(|state| u64::from(state.code()) == code)
            .ok_or(ProtocolError::INVALID_INPUT)
    }
}

/// Why a task failed or was cancelled.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent task end reasons are a closed, durable vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentTaskReason {
    /// A `cancel` request ended it.
    Requested,
    /// Its creator no longer has a complete authorization path to the Agent.
    Unauthorized,
    /// The Agent reported the failure.
    AgentFailed,
    /// The Agent instance holding it ended.
    AgentLost,
    /// The operator stopped or removed the Agent.
    AgentStopped,
    /// The task exceeded an event, tool, or artifact bound.
    Limit,
}

impl AgentTaskReason {
    /// Every reason in wire-code order.
    pub const ALL: [Self; 6] = [
        Self::Requested,
        Self::Unauthorized,
        Self::AgentFailed,
        Self::AgentLost,
        Self::AgentStopped,
        Self::Limit,
    ];

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Unauthorized => "unauthorized",
            Self::AgentFailed => "agent_failed",
            Self::AgentLost => "agent_lost",
            Self::AgentStopped => "agent_stopped",
            Self::Limit => "limit",
        }
    }

    /// Returns the reason with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.name() == name)
    }

    /// Returns the stable durable code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Requested => 0,
            Self::Unauthorized => 1,
            Self::AgentFailed => 2,
            Self::AgentLost => 3,
            Self::AgentStopped => 4,
            Self::Limit => 5,
        }
    }

    /// Returns the reason with this durable code.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an unknown code.
    pub fn from_code(code: u64) -> Result<Self, ProtocolError> {
        Self::ALL
            .into_iter()
            .find(|reason| u64::from(reason.code()) == code)
            .ok_or(ProtocolError::INVALID_INPUT)
    }
}
