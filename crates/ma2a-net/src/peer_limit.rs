//! Per-peer and Runtime-wide bounds on inbound work, held by a permit until it drops.

use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::EndpointId;

#[derive(Clone, Debug)]
pub(crate) struct PeerLimiter {
    per_peer: usize,
    global: usize,
    state: Arc<std::sync::Mutex<LimitState>>,
}

#[derive(Debug, Default)]
struct LimitState {
    total: usize,
    peers: BTreeMap<EndpointId, usize>,
}

/// RAII capacity held for the whole life of one admitted stream or connection.
#[derive(Debug)]
pub struct PeerPermit {
    limiter: PeerLimiter,
    peer: EndpointId,
}

impl PeerLimiter {
    pub(crate) fn new(per_peer: usize, global: usize) -> Self {
        Self {
            per_peer,
            global,
            state: Arc::default(),
        }
    }

    pub(crate) fn try_acquire(&self, peer: EndpointId) -> Option<PeerPermit> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or_default();
        if state.total >= self.global || peer_count >= self.per_peer {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer, peer_count + 1);
        drop(state);
        Some(PeerPermit {
            limiter: self.clone(),
            peer,
        })
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total
    }
}

impl Drop for PeerPermit {
    fn drop(&mut self) {
        let mut state = self
            .limiter
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.total = state.total.saturating_sub(1);
        match state.peers.get(&self.peer).copied() {
            Some(1) => {
                state.peers.remove(&self.peer);
            }
            Some(count) => {
                state.peers.insert(self.peer, count - 1);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PeerLimiter;
    use crate::EndpointSecret;

    #[test]
    fn per_peer_and_global_bounds_release_on_drop() {
        let (per_peer, global) = (4, 16);
        let limiter = PeerLimiter::new(per_peer, global);
        let peer = EndpointSecret::generate().endpoint_id();
        let mut held = (0..per_peer)
            .map(|_| limiter.try_acquire(peer).expect("within bound"))
            .collect::<Vec<_>>();
        assert!(limiter.try_acquire(peer).is_none());
        held.pop();
        held.push(limiter.try_acquire(peer).expect("released capacity"));
        let others = (per_peer..global)
            .map(|_| {
                limiter
                    .try_acquire(EndpointSecret::generate().endpoint_id())
                    .expect("within global bound")
            })
            .collect::<Vec<_>>();
        assert!(
            limiter
                .try_acquire(EndpointSecret::generate().endpoint_id())
                .is_none()
        );
        drop(others);
        drop(held);
        assert_eq!(limiter.active(), 0);
    }
}
