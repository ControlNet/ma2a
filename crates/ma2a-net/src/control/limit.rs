use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::EndpointId;

use super::{CONTROL_GLOBAL_LIMIT, CONTROL_PER_PEER_LIMIT};

#[derive(Clone, Debug, Default)]
pub(super) struct ControlLimiter(Arc<std::sync::Mutex<LimitState>>);

#[derive(Debug, Default)]
struct LimitState {
    total: usize,
    peers: BTreeMap<EndpointId, usize>,
}

#[derive(Debug)]
pub(super) struct ControlPermit {
    limiter: ControlLimiter,
    peer: EndpointId,
}

impl ControlLimiter {
    pub(super) fn try_acquire(&self, peer: EndpointId) -> Option<ControlPermit> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or_default();
        if state.total >= CONTROL_GLOBAL_LIMIT || peer_count >= CONTROL_PER_PEER_LIMIT {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer, peer_count + 1);
        drop(state);
        Some(ControlPermit {
            limiter: self.clone(),
            peer,
        })
    }
}

impl Drop for ControlPermit {
    fn drop(&mut self) {
        let mut state = self
            .limiter
            .0
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
    use super::{CONTROL_GLOBAL_LIMIT, CONTROL_PER_PEER_LIMIT, ControlLimiter};
    use crate::EndpointSecret;

    #[test]
    fn peer_limit_rejects_excess_and_drop_releases_capacity() {
        // Given
        let limiter = ControlLimiter::default();
        let peer = EndpointSecret::generate().endpoint_id();
        let mut permits = (0..CONTROL_PER_PEER_LIMIT)
            .map(|_| limiter.try_acquire(peer).expect("within per-peer limit"))
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), CONTROL_PER_PEER_LIMIT);

        // When
        let excess = limiter.try_acquire(peer);
        permits.pop();
        let replacement = limiter.try_acquire(peer);

        // Then
        assert!(excess.is_none());
        assert!(replacement.is_some());
    }

    #[test]
    fn global_limit_rejects_excess_and_drop_releases_capacity() {
        // Given
        let limiter = ControlLimiter::default();
        let mut permits = (0..CONTROL_GLOBAL_LIMIT)
            .map(|_| {
                limiter
                    .try_acquire(EndpointSecret::generate().endpoint_id())
                    .expect("within global limit")
            })
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), CONTROL_GLOBAL_LIMIT);
        let excess_peer = EndpointSecret::generate().endpoint_id();

        // When
        let excess = limiter.try_acquire(excess_peer);
        permits.pop();
        let replacement = limiter.try_acquire(excess_peer);

        // Then
        assert!(excess.is_none());
        assert!(replacement.is_some());
    }
}
