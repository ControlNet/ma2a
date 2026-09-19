//! Authoritative client snapshot without secret or authorization diagnostics.

use super::{ApiError, MAX_COLLECTION_ITEMS, snapshot_state::SnapshotState};
use ma2a_core::EndpointId;

mod connection;
mod control_round;
mod member;
mod relay;
mod space;
mod value;

pub use connection::{ConnectionObservationView, ConnectionView};
pub use control_round::{ControlRoundView, MAX_RETAINED_CONTROL_ROUNDS};
pub use relay::{PrivateRelayCandidateView, PublicRelayFallbackView};
pub use member::{SnapshotSpaceView, SpaceChainHead, SpaceMemberView};
pub use space::SpaceView;
pub(crate) use value::{endpoint_value, space_value, ui_auth_value};

/// Runtime Endpoint information safe for an authorized local client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointView {
    pub(crate) id: EndpointId,
    pub(crate) runtime_version: String,
    pub(crate) online: bool,
}

impl EndpointView {
    /// Creates bounded public Endpoint information.
    ///
    /// # Errors
    /// Returns invalid input when the Runtime version length is outside the bound.
    pub fn new(id: EndpointId, runtime_version: &str, online: bool) -> Result<Self, ApiError> {
        if runtime_version.is_empty() || runtime_version.len() > 128 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                id,
                runtime_version: runtime_version.to_owned(),
                online,
            })
        }
    }
}


/// Control synchronization state keyed only by peer Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSyncView {
    pub(crate) peers: Vec<EndpointId>,
}

impl ControlSyncView {
    /// Creates bounded control-sync state.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 peers are supplied.
    pub fn new(peers: Vec<EndpointId>) -> Result<Self, ApiError> {
        if peers.len() > MAX_COLLECTION_ITEMS {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self { peers })
        }
    }
}


/// Local provider-role state and Iroh public-relay observation, kept apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRelayStateView {
    pub(crate) private_relay_provider_running: bool,
    pub(crate) public_relay_connected: bool,
}

impl ObservedRelayStateView {
    /// Creates observed Private and Public Relay status.
    /// `private_relay_provider_running` is a local service-role fact: this Runtime
    /// currently hosts its embedded Private Relay. `public_relay_connected` is an
    /// Iroh transport observation for this Endpoint. Neither may be used to
    /// reconstruct `ReachabilityView::state`, which the Runtime owns.
    pub const fn new(private_relay_provider_running: bool, public_relay_connected: bool) -> Self {
        Self {
            private_relay_provider_running,
            public_relay_connected,
        }
    }
}

/// Current direct and relay reachability summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachabilityView {
    pub(crate) state: &'static str,
    pub(crate) direct: bool,
    pub(crate) relayed: bool,
}

impl ReachabilityView {
    /// Creates reachability state from the Runtime's own `RelayReachability`.
    ///
    /// `state` is authoritative. `direct` and `relayed` remain separate observed
    /// path facts and must not be used to reconstruct `state` elsewhere.
    ///
    /// # Errors
    /// Returns invalid input for a state outside the closed set.
    pub fn new(state: &'static str, direct: bool, relayed: bool) -> Result<Self, ApiError> {
        if !matches!(
            state,
            "NoActiveSpaces" | "DegradedNoCommonHome" | "AwaitingIrohHome" | "IrohHomeConnected"
        ) {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            state,
            direct,
            relayed,
        })
    }
}

/// Recent bounded Echo outcome totals without payload retention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoSummaryView {
    pub(crate) successes: u32,
    pub(crate) failures: u32,
}

impl EchoSummaryView {
    /// Creates recent Echo outcome totals.
    pub const fn new(successes: u32, failures: u32) -> Self {
        Self {
            successes,
            failures,
        }
    }
}

/// UI authentication state without verifier, bearer, or digest material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiAuthView {
    pub(crate) initialized: bool,
    pub(crate) password_set: bool,
    pub(crate) active_sessions: u32,
}

impl UiAuthView {
    /// Creates public UI authentication state.
    pub const fn new(initialized: bool, password_set: bool, active_sessions: u32) -> Self {
        Self {
            initialized,
            password_set,
            active_sessions,
        }
    }
}

/// Authoritative full-state projection used after startup, disconnect, or revision gaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    revision: u64,
    endpoint: EndpointView,
    spaces: Vec<SnapshotSpaceView>,
    control_sync: ControlSyncView,
    connections: Vec<ConnectionView>,
    private_relay_candidates: Vec<PrivateRelayCandidateView>,
    public_relay_fallbacks: Vec<PublicRelayFallbackView>,
    control_rounds: Vec<ControlRoundView>,
    observed_relay_state: ObservedRelayStateView,
    reachability: ReachabilityView,
    recent_echo_summary: EchoSummaryView,
    ui_auth: UiAuthView,
}

impl RuntimeSnapshot {
    /// Creates a snapshot after enforcing all collection bounds.
    ///
    /// # Errors
    /// Returns invalid input when any collection exceeds 256 entities.
    pub fn new(
        header: SnapshotHeader,
        collections: SnapshotCollections,
        state: SnapshotState,
    ) -> Result<Self, ApiError> {
        if collections.spaces.len() > MAX_COLLECTION_ITEMS
            || collections.private_relay_candidates.len() > MAX_COLLECTION_ITEMS
            || collections.control_sync.peers.len() > MAX_COLLECTION_ITEMS
            || collections.connections.len() > MAX_COLLECTION_ITEMS
        {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            revision: header.revision,
            endpoint: header.endpoint,
            spaces: collections.spaces,
            control_sync: collections.control_sync,
            connections: collections.connections,
            private_relay_candidates: collections.private_relay_candidates,
            public_relay_fallbacks: collections.public_relay_fallbacks,
            control_rounds: collections.control_rounds,
            observed_relay_state: state.observed_relay_state,
            reachability: state.reachability,
            recent_echo_summary: state.recent_echo_summary,
            ui_auth: state.ui_auth,
        })
    }

    /// Returns the authoritative state revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn spaces(&self) -> &[SnapshotSpaceView] {
        &self.spaces
    }
}

/// Revision and Endpoint header for snapshot construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotHeader {
    revision: u64,
    endpoint: EndpointView,
}

impl SnapshotHeader {
    /// Creates a snapshot header.
    pub const fn new(revision: u64, endpoint: EndpointView) -> Self {
        Self { revision, endpoint }
    }
}

/// Bounded collections grouped for snapshot construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCollections {
    pub(crate) spaces: Vec<SnapshotSpaceView>,
    pub(crate) control_sync: ControlSyncView,
    pub(crate) connections: Vec<ConnectionView>,
    pub(crate) private_relay_candidates: Vec<PrivateRelayCandidateView>,
    pub(crate) public_relay_fallbacks: Vec<PublicRelayFallbackView>,
    pub(crate) control_rounds: Vec<ControlRoundView>,
}

impl SnapshotCollections {
    /// Groups the bounded snapshot collections.
    ///
    /// # Errors
    /// Returns invalid input when a collection exceeds 256 entities.
    #[expect(
        clippy::too_many_arguments,
        reason = "the snapshot groups five independently bounded wire collections"
    )]
    pub fn new(
        spaces: Vec<SnapshotSpaceView>,
        control_sync: ControlSyncView,
        connections: Vec<ConnectionView>,
        private_relay_candidates: Vec<PrivateRelayCandidateView>,
    public_relay_fallbacks: Vec<PublicRelayFallbackView>,
        control_rounds: Vec<ControlRoundView>,
    ) -> Result<Self, ApiError> {
        if spaces.len() > MAX_COLLECTION_ITEMS
            || connections.len() > MAX_COLLECTION_ITEMS
            || private_relay_candidates.len() > MAX_COLLECTION_ITEMS
            || public_relay_fallbacks.len() > MAX_COLLECTION_ITEMS
            || control_rounds.len() > MAX_RETAINED_CONTROL_ROUNDS
        {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                spaces,
                control_sync,
                connections,
                private_relay_candidates,
                public_relay_fallbacks,
                control_rounds,
            })
        }
    }
}
