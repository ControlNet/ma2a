use std::collections::BTreeSet;

use ma2a_core::{AddressEndpointDataV1, EndpointId, SpaceId};
use ma2a_net::EndpointAddr;

use crate::reachability::RelayReachabilityState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectivityKind {
    DirectOnly,
}

/// Truthful current transport capability for an Endpoint without relay configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Connectivity(ConnectivityKind);

impl Connectivity {
    pub(crate) const DIRECT_ONLY: Self = Self(ConnectivityKind::DirectOnly);

    /// Returns whether only direct Iroh addressing is currently configured.
    pub const fn is_direct_only(self) -> bool {
        matches!(self.0, ConnectivityKind::DirectOnly)
    }
}

/// Authoritative in-process Runtime lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) endpoint_addr: EndpointAddr,
    pub(crate) endpoint_data: AddressEndpointDataV1,
    pub(crate) boot_id: [u8; 16],
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<SpaceId>,
    pub(crate) ready: bool,
    pub(crate) connectivity: Connectivity,
    pub(crate) direct_reachable: bool,
    pub(crate) relay: RelayReachabilityState,
}

impl RuntimeStatus {
    /// Returns the one persistent Endpoint identity.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }

    /// Returns current Iroh addressing for direct connection attempts.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint_addr.clone()
    }

    /// Returns the current boot identifier.
    pub const fn boot_id(&self) -> [u8; 16] {
        self.boot_id
    }

    /// Returns the current monotonic state revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the number of locally valid Space memberships.
    pub fn membership_count(&self) -> usize {
        self.memberships.len()
    }

    /// Returns whether at least one verified local Space enables normal protocols.
    pub fn normal_protocols_eligible(&self) -> bool {
        !self.memberships.is_empty()
    }

    /// Returns whether startup completed successfully.
    pub const fn is_ready(&self) -> bool {
        self.ready
    }

    /// Returns truthful current connectivity capability.
    pub const fn connectivity(&self) -> Connectivity {
        self.connectivity
    }

    /// Returns relay-backed reachability derived from Iroh-observed state.
    pub const fn relay_reachability(&self) -> ma2a_core::RelayReachability {
        self.relay.reachability()
    }

    /// Iterates Iroh-reported home relay URLs accepted from the supplied map.
    pub fn observed_home_relays(&self) -> impl Iterator<Item = &str> {
        self.relay.observed_home_relays()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    Ready,
    MembershipsChanged,
    ShuttingDown,
}

/// Bounded best-effort Runtime actor event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeEvent {
    revision: u64,
    kind: EventKind,
}

impl RuntimeEvent {
    pub(crate) const fn ready(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::Ready,
        }
    }

    pub(crate) const fn memberships_changed(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::MembershipsChanged,
        }
    }

    pub(crate) const fn shutting_down(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::ShuttingDown,
        }
    }

    /// Returns the event's monotonic state revision.
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Returns whether startup reached ready state.
    pub const fn is_ready(self) -> bool {
        matches!(self.kind, EventKind::Ready)
    }
}

/// Deterministic graceful-shutdown evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub(crate) joined_tasks: usize,
    pub(crate) endpoint_closed: bool,
    pub(crate) revision: u64,
}

impl ShutdownReport {
    /// Returns the number of owned actor and blocking tasks joined.
    pub const fn joined_tasks(self) -> usize {
        self.joined_tasks
    }

    /// Returns whether Iroh confirmed Endpoint closure.
    pub const fn endpoint_closed(self) -> bool {
        self.endpoint_closed
    }

    /// Returns the final persisted revision.
    pub const fn revision(self) -> u64 {
        self.revision
    }
}
