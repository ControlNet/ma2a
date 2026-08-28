//! Grouped scalar state used to construct an authoritative snapshot.

use super::snapshot::{EchoSummaryView, ObservedRelayStateView, ReachabilityView, UiAuthView};

/// Scalar snapshot state grouped for snapshot construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotState {
    pub(crate) observed_relay_state: ObservedRelayStateView,
    pub(crate) reachability: ReachabilityView,
    pub(crate) recent_echo_summary: EchoSummaryView,
    pub(crate) ui_auth: UiAuthView,
}

impl SnapshotState {
    /// Groups scalar snapshot state.
    pub const fn new(network: NetworkSnapshotState, client: ClientSnapshotState) -> Self {
        Self {
            observed_relay_state: network.observed_relay_state,
            reachability: network.reachability,
            recent_echo_summary: client.recent_echo_summary,
            ui_auth: client.ui_auth,
        }
    }
}

/// Relay and reachability state grouped for snapshot construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSnapshotState {
    observed_relay_state: ObservedRelayStateView,
    reachability: ReachabilityView,
}

impl NetworkSnapshotState {
    /// Creates grouped network snapshot state.
    pub const fn new(
        observed_relay_state: ObservedRelayStateView,
        reachability: ReachabilityView,
    ) -> Self {
        Self {
            observed_relay_state,
            reachability,
        }
    }
}

/// Echo and UI authentication state grouped for snapshot construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSnapshotState {
    recent_echo_summary: EchoSummaryView,
    ui_auth: UiAuthView,
}

impl ClientSnapshotState {
    /// Creates grouped client snapshot state.
    pub const fn new(recent_echo_summary: EchoSummaryView, ui_auth: UiAuthView) -> Self {
        Self {
            recent_echo_summary,
            ui_auth,
        }
    }
}
