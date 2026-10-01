//! Closed Runtime, Endpoint, Space, and built-in service ontology.

use crate::{EndpointId, SpaceId};

/// The complete closed set of built-in services.
///
/// There is no runtime registration: every service is known at compile time and
/// every dispatch over this type is exhaustive.
#[expect(
    clippy::exhaustive_enums,
    reason = "canonical dispatch must update every match when a service is added"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceKind {
    /// The bounded encrypted Echo service.
    Echo,
    /// Bounded remote filesystem operations on exported roots.
    Files,
    /// Streaming large-content transfer with integrity verification.
    Transfer,
    /// Non-interactive one-shot program execution.
    Exec,
    /// Interactive terminal sessions.
    Shell,
    /// Endpoint-targeted TCP byte-stream proxying.
    Proxy,
    /// Durable workloads that outlive the operation that started them.
    Process,
    /// Tasks for Agents the target Endpoint hosts.
    Agent,
}

impl ServiceKind {
    /// Every built-in service in wire-code order.
    pub const ALL: [Self; 8] = [
        Self::Echo,
        Self::Files,
        Self::Transfer,
        Self::Exec,
        Self::Shell,
        Self::Proxy,
        Self::Process,
        Self::Agent,
    ];

    /// Returns the stable one-byte wire code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Echo => 1,
            Self::Files => 2,
            Self::Transfer => 3,
            Self::Exec => 4,
            Self::Shell => 5,
            Self::Proxy => 6,
            Self::Process => 7,
            Self::Agent => 8,
        }
    }

    /// Decodes a stable one-byte wire code.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Echo),
            2 => Some(Self::Files),
            3 => Some(Self::Transfer),
            4 => Some(Self::Exec),
            5 => Some(Self::Shell),
            6 => Some(Self::Proxy),
            7 => Some(Self::Process),
            8 => Some(Self::Agent),
            _ => None,
        }
    }

    /// Returns the stable lowercase name used by the local API and CLI.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Files => "files",
            Self::Transfer => "transfer",
            Self::Exec => "exec",
            Self::Shell => "shell",
            Self::Proxy => "proxy",
            Self::Process => "process",
            Self::Agent => "agent",
        }
    }

    /// Parses the stable lowercase name used by the local API and CLI.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// The one persistent Endpoint identity owned by a Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeIdentity {
    endpoint_id: EndpointId,
}

impl RuntimeIdentity {
    /// Creates a Runtime identity independently of all Space memberships.
    pub const fn new(endpoint_id: EndpointId) -> Self {
        Self { endpoint_id }
    }

    /// Returns the Runtime's single Endpoint identity.
    pub const fn endpoint_id(self) -> EndpointId {
        self.endpoint_id
    }
}

/// One independent membership relation between a Space and an Endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceMembership {
    space_id: SpaceId,
    endpoint_id: EndpointId,
}

impl SpaceMembership {
    /// Creates a membership relation without changing Endpoint identity.
    pub const fn new(space_id: SpaceId, endpoint_id: EndpointId) -> Self {
        Self {
            space_id,
            endpoint_id,
        }
    }

    /// Returns the independent Space identifier.
    pub const fn space_id(self) -> SpaceId {
        self.space_id
    }

    /// Returns the member Endpoint identifier.
    pub const fn endpoint_id(self) -> EndpointId {
        self.endpoint_id
    }
}
