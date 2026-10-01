//! Process lifecycle states and end reasons.

use crate::ProtocolError;

/// The lifecycle state of one Process.
///
/// `Running` is only ever asserted by an authenticated supervisor on a live connection;
/// it is never inferred from an operating system process identifier.
#[expect(
    clippy::exhaustive_enums,
    reason = "Process states are a closed, durable vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProcessState {
    /// Recorded with its `RequestId` binding; no child can exist yet.
    Created,
    /// About to be told to spawn by its supervisor.
    Starting,
    /// Its supervisor reported the child running.
    Running,
    /// The child ended by itself.
    Exited,
    /// MA2A ended the child on request or on withdrawn authorization.
    Terminated,
    /// The program could not start, or the start was interrupted before any child existed.
    Failed,
    /// The outcome cannot be proven: the supervisor is gone and recorded no end.
    Lost,
}

impl ProcessState {
    /// Every state in wire-code order.
    pub const ALL: [Self; 7] = [
        Self::Created,
        Self::Starting,
        Self::Running,
        Self::Exited,
        Self::Terminated,
        Self::Failed,
        Self::Lost,
    ];

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }

    /// Returns the state with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }

    /// Returns whether no further transition is possible.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Terminated | Self::Failed | Self::Lost
        )
    }

    /// Returns whether the lifecycle permits moving from `self` to `next`.
    pub const fn permits(self, next: Self) -> bool {
        match (self, next) {
            // A start that already spawned may end before `running` was recorded.
            (Self::Created, Self::Starting | Self::Failed)
            | (
                Self::Starting,
                Self::Running | Self::Exited | Self::Terminated | Self::Failed | Self::Lost,
            )
            | (Self::Running, Self::Exited | Self::Terminated | Self::Lost) => true,
            _ => false,
        }
    }

    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::Created => 0,
            Self::Starting => 1,
            Self::Running => 2,
            Self::Exited => 3,
            Self::Terminated => 4,
            Self::Failed => 5,
            Self::Lost => 6,
        }
    }

    pub(crate) fn from_code(code: u64) -> Result<Self, ProtocolError> {
        Self::ALL
            .into_iter()
            .find(|state| state.code() == code)
            .ok_or(ProtocolError::INVALID_INPUT)
    }
}

/// Why a Process reached a terminal state other than exiting by itself.
#[expect(
    clippy::exhaustive_enums,
    reason = "Process end reasons are a closed, durable vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessReason {
    /// The program could not be started.
    SpawnFailed,
    /// The start was interrupted before its supervisor was told to spawn.
    Interrupted,
    /// A `terminate` request ended it.
    Requested,
    /// Its creator no longer has a complete authorization path.
    Unauthorized,
    /// Its supervisor disappeared without recording an end.
    SupervisorLost,
}

impl ProcessReason {
    /// Every reason in wire-code order.
    pub const ALL: [Self; 5] = [
        Self::SpawnFailed,
        Self::Interrupted,
        Self::Requested,
        Self::Unauthorized,
        Self::SupervisorLost,
    ];

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::SpawnFailed => "spawn_failed",
            Self::Interrupted => "interrupted",
            Self::Requested => "requested",
            Self::Unauthorized => "unauthorized",
            Self::SupervisorLost => "supervisor_lost",
        }
    }

    /// Returns the reason with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.name() == name)
    }

    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::SpawnFailed => 0,
            Self::Interrupted => 1,
            Self::Requested => 2,
            Self::Unauthorized => 3,
            Self::SupervisorLost => 4,
        }
    }

    pub(crate) fn from_code(code: u64) -> Result<Self, ProtocolError> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.code() == code)
            .ok_or(ProtocolError::INVALID_INPUT)
    }
}
