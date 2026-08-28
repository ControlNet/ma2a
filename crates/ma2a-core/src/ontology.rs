//! Closed Phase 1 Runtime, Endpoint, Space, and service ontology.

use crate::{EndpointId, SpaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceCode {
    Echo,
}

/// The complete Phase 1 service set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceKind(ServiceCode);

impl ServiceKind {
    /// The bounded encrypted Echo service.
    pub const ECHO: Self = Self(ServiceCode::Echo);
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
