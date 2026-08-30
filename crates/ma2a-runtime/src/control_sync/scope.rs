use std::collections::BTreeSet;

use ma2a_core::EndpointId;
use ma2a_net::select_peer_window;

const MAX_TARGETED_PEERS_PER_ROUND: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ControlRoundScope {
    global: bool,
    peers: BTreeSet<EndpointId>,
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
        Self {
            global: true,
            peers: BTreeSet::new(),
        }
    }

    pub(crate) fn peer(peer: EndpointId) -> Self {
        Self {
            global: false,
            peers: BTreeSet::from([peer]),
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.global |= other.global;
        self.peers.extend(other.peers);
    }

    pub(crate) fn waiter_peer(&self) -> Option<EndpointId> {
        if self.global || self.peers.len() != 1 {
            return None;
        }
        self.peers.iter().next().copied()
    }

    pub(crate) fn contains_peer(&self, peer: EndpointId) -> bool {
        self.peers.contains(&peer)
    }

    pub(crate) fn take_round(&mut self) -> Option<Self> {
        if !self.peers.is_empty() {
            let selected = self
                .peers
                .iter()
                .copied()
                .take(MAX_TARGETED_PEERS_PER_ROUND)
                .collect::<BTreeSet<_>>();
            self.peers.retain(|peer| !selected.contains(peer));
            return Some(Self {
                global: false,
                peers: selected,
            });
        }
        if self.global {
            self.global = false;
            return Some(Self::all());
        }
        None
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.global && self.peers.is_empty()
    }
}

impl ControlRoundRequest {
    pub(crate) fn select(&self, eligible: &[EndpointId]) -> Vec<EndpointId> {
        if self.scope.global {
            return select_peer_window(self.local_endpoint_id, eligible, self.rotation);
        }
        eligible
            .iter()
            .copied()
            .filter(|peer| self.scope.peers.contains(peer))
            .collect()
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
