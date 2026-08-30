use std::collections::BTreeSet;

use ma2a_core::EndpointId;
use ma2a_net::select_peer_window;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ControlRoundScope {
    All,
    Peers(BTreeSet<EndpointId>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ControlRoundTrigger {
    Startup,
    ManifestAdvanced,
    AddressAdvanced,
    RelayAdvanced,
    EnrollmentCompleted,
    Explicit(ControlRoundScope),
    Periodic,
}

#[derive(Clone, Debug)]
pub(crate) struct ControlRoundRequest {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) rotation: usize,
    pub(crate) now_ms: u64,
    pub(crate) scope: ControlRoundScope,
}

#[derive(Debug)]
pub(crate) struct ControlRoundOutcome {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<ma2a_core::SpaceId>,
    pub(crate) synchronized_peers: BTreeSet<EndpointId>,
    pub(crate) changes: super::ControlChanges,
}

impl ControlRoundScope {
    pub(crate) const fn all() -> Self {
        Self::All
    }

    pub(crate) fn peer(peer: EndpointId) -> Self {
        Self::Peers(BTreeSet::from([peer]))
    }

    pub(crate) fn merge(&mut self, other: Self) {
        match (&mut *self, other) {
            (Self::All, Self::All | Self::Peers(_)) | (Self::Peers(_), Self::All) => {
                *self = Self::All;
            }
            (Self::Peers(current), Self::Peers(peers)) => current.extend(peers),
        }
    }
}

impl ControlRoundRequest {
    pub(crate) fn select(&self, eligible: &[EndpointId]) -> Vec<EndpointId> {
        match &self.scope {
            ControlRoundScope::All => {
                select_peer_window(self.local_endpoint_id, eligible, self.rotation)
            }
            ControlRoundScope::Peers(requested) => eligible
                .iter()
                .copied()
                .filter(|peer| requested.contains(peer))
                .collect(),
        }
    }
}

impl ControlRoundTrigger {
    pub(crate) fn scope(self) -> ControlRoundScope {
        match self {
            Self::Explicit(scope) => scope,
            Self::Startup
            | Self::ManifestAdvanced
            | Self::AddressAdvanced
            | Self::RelayAdvanced
            | Self::EnrollmentCompleted
            | Self::Periodic => ControlRoundScope::all(),
        }
    }
}
