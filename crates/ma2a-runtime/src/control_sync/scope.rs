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
    pub(crate) dialed_peers: BTreeSet<EndpointId>,
    pub(crate) synchronized_peers: BTreeSet<EndpointId>,
    pub(crate) changes: super::ControlChanges,
    /// Peers were dialed, yet none of them answered every request. What did arrive is
    /// committed, and `changes` carries it, but the round itself failed.
    pub(crate) incomplete: bool,
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

    pub(crate) const fn is_global(&self) -> bool {
        self.global
    }

    pub(crate) fn contains_peer(&self, peer: EndpointId) -> bool {
        self.peers.contains(&peer)
    }

    /// Takes the next round: targeted peers first, unless `global_first` gives a pending global
    /// round its turn.
    pub(crate) fn take_round(&mut self, global_first: bool) -> Option<Self> {
        if global_first && self.global {
            self.global = false;
            return Some(Self::all());
        }
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
            .filter(|peer| *peer != self.local_endpoint_id && self.scope.peers.contains(peer))
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

#[cfg(test)]
mod tests {
    use ma2a_net::EndpointSecret;

    use super::{ControlRoundRequest, ControlRoundScope};

    #[test]
    fn targeted_scope_excludes_local_endpoint_id()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let local = EndpointSecret::parse(&[0x11; 32])?.endpoint_id();
        let other = EndpointSecret::parse(&[0x22; 32])?.endpoint_id();

        let selected = ControlRoundRequest {
            local_endpoint_id: local,
            rotation: 0,
            now_ms: 0,
            scope: ControlRoundScope::peer(local),
        }
        .select(&[local, other]);

        assert!(
            selected.is_empty(),
            "self must never be selected: {selected:?}"
        );
        Ok(())
    }
}
