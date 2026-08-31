//! Authoritative client snapshot without secret or authorization diagnostics.

use ma2a_core::{EndpointId, SpaceId};
use serde_json::{Value, json};

use super::{
    ApiError, MAX_COLLECTION_ITEMS, codec_fields::encode_hex, snapshot_state::SnapshotState,
};

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

/// Bounded Space summary without membership authorization diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceView {
    pub(crate) id: SpaceId,
    pub(crate) name: String,
    pub(crate) member_count: u32,
}

impl SpaceView {
    /// Creates a bounded Space summary.
    ///
    /// # Errors
    /// Returns invalid input when the Space name length is outside the bound.
    pub fn new(id: SpaceId, name: &str, member_count: u32) -> Result<Self, ApiError> {
        if name.is_empty() || name.len() > 128 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                id,
                name: name.to_owned(),
                member_count,
            })
        }
    }

    pub(crate) const fn id(&self) -> SpaceId {
        self.id
    }
}

/// Control synchronization state keyed only by peer Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSyncView {
    pub(crate) peers: Vec<EndpointId>,
    pub(crate) synchronized: bool,
}

impl ControlSyncView {
    /// Creates bounded control-sync state.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 peers are supplied.
    pub fn new(peers: Vec<EndpointId>, synchronized: bool) -> Result<Self, ApiError> {
        if peers.len() > MAX_COLLECTION_ITEMS {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                peers,
                synchronized,
            })
        }
    }
}

/// One bounded relay candidate exposed without broad topology data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCandidateView {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) relay_kind: String,
    pub(crate) eligible: bool,
}

impl RelayCandidateView {
    /// Creates one relay candidate without topology expansion.
    ///
    /// # Errors
    /// Returns invalid input when the relay kind length is outside the bound.
    pub fn new(
        endpoint_id: EndpointId,
        relay_kind: &str,
        eligible: bool,
    ) -> Result<Self, ApiError> {
        if relay_kind.is_empty() || relay_kind.len() > 32 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                endpoint_id,
                relay_kind: relay_kind.to_owned(),
                eligible,
            })
        }
    }
}

/// Locally observed relay state, distinct from configuration and advertisements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRelayStateView {
    pub(crate) private_relay_online: bool,
    pub(crate) public_relay_online: bool,
}

impl ObservedRelayStateView {
    /// Creates observed Private and Public Relay status.
    pub const fn new(private_relay_online: bool, public_relay_online: bool) -> Self {
        Self {
            private_relay_online,
            public_relay_online,
        }
    }
}

/// Current direct and relay reachability summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachabilityView {
    pub(crate) direct: bool,
    pub(crate) relayed: bool,
}

impl ReachabilityView {
    /// Creates direct and relayed reachability state.
    pub const fn new(direct: bool, relayed: bool) -> Self {
        Self { direct, relayed }
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
    spaces: Vec<SpaceView>,
    control_sync: ControlSyncView,
    relay_candidates: Vec<RelayCandidateView>,
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
            || collections.relay_candidates.len() > MAX_COLLECTION_ITEMS
            || collections.control_sync.peers.len() > MAX_COLLECTION_ITEMS
        {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            revision: header.revision,
            endpoint: header.endpoint,
            spaces: collections.spaces,
            control_sync: collections.control_sync,
            relay_candidates: collections.relay_candidates,
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

    pub(crate) fn spaces(&self) -> &[SpaceView] {
        &self.spaces
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({
            "revision": self.revision,
            "endpoint": endpoint_value(&self.endpoint),
            "spaces": self.spaces.iter().map(space_value).collect::<Vec<_>>(),
            "control_sync": {"peer_endpoint_ids": self.control_sync.peers.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>(), "synchronized": self.control_sync.synchronized},
            "relay_candidates": self.relay_candidates.iter().map(relay_candidate_value).collect::<Vec<_>>(),
            "observed_relay_state": {"private_relay_online": self.observed_relay_state.private_relay_online, "public_relay_online": self.observed_relay_state.public_relay_online},
            "reachability": {"direct": self.reachability.direct, "relayed": self.reachability.relayed},
            "recent_echo_summary": {"successes": self.recent_echo_summary.successes, "failures": self.recent_echo_summary.failures},
            "ui_auth": ui_auth_value(&self.ui_auth),
        })
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
    pub(crate) spaces: Vec<SpaceView>,
    pub(crate) control_sync: ControlSyncView,
    pub(crate) relay_candidates: Vec<RelayCandidateView>,
}

impl SnapshotCollections {
    /// Groups the bounded snapshot collections.
    ///
    /// # Errors
    /// Returns invalid input when a collection exceeds 256 entities.
    pub fn new(
        spaces: Vec<SpaceView>,
        control_sync: ControlSyncView,
        relay_candidates: Vec<RelayCandidateView>,
    ) -> Result<Self, ApiError> {
        if spaces.len() > MAX_COLLECTION_ITEMS || relay_candidates.len() > MAX_COLLECTION_ITEMS {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                spaces,
                control_sync,
                relay_candidates,
            })
        }
    }
}

pub(crate) fn endpoint_value(endpoint: &EndpointView) -> Value {
    json!({"endpoint_id": encode_hex(endpoint.id.as_bytes()), "runtime_version": endpoint.runtime_version, "online": endpoint.online})
}

pub(crate) fn space_value(space: &SpaceView) -> Value {
    json!({"space_id": encode_hex(space.id.as_bytes()), "name": space.name, "member_count": space.member_count})
}

pub(crate) fn ui_auth_value(auth: &UiAuthView) -> Value {
    json!({"initialized": auth.initialized, "password_set": auth.password_set, "active_sessions": auth.active_sessions})
}

fn relay_candidate_value(candidate: &RelayCandidateView) -> Value {
    json!({"endpoint_id": encode_hex(candidate.endpoint_id.as_bytes()), "relay_kind": candidate.relay_kind, "eligible": candidate.eligible})
}
