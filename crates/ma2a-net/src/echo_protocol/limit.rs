use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::EndpointId;

use super::{ECHO_GLOBAL_LIMIT, ECHO_PER_PEER_LIMIT};

#[derive(Clone, Debug, Default)]
pub(super) struct EchoLimiter(Arc<std::sync::Mutex<LimitState>>);

#[derive(Debug, Default)]
struct LimitState {
    total: usize,
    peers: BTreeMap<EndpointId, usize>,
}

#[derive(Debug)]
pub(super) struct EchoPermit {
    limiter: EchoLimiter,
    peer: EndpointId,
}

impl EchoLimiter {
    pub(super) fn try_acquire(&self, peer: EndpointId) -> Option<EchoPermit> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or_default();
        if state.total >= ECHO_GLOBAL_LIMIT || peer_count >= ECHO_PER_PEER_LIMIT {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer, peer_count + 1);
        drop(state);
        Some(EchoPermit {
            limiter: self.clone(),
            peer,
        })
    }
}

impl Drop for EchoPermit {
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
    use ma2a_core::EndpointId;

    use super::{ECHO_GLOBAL_LIMIT, EchoLimiter};
    use crate::EndpointSecret;

    const PEER: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66,
    ];

    #[test]
    fn seventeenth_peer_permit_is_rejected_and_drop_releases_capacity() {
        // Given
        let limiter = EchoLimiter::default();
        let peer = EndpointId::try_from(PEER.as_slice()).expect("valid peer");
        let mut permits = (0..16)
            .map(|_| limiter.try_acquire(peer).expect("within per-peer limit"))
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), 16);

        // When
        let excess = limiter.try_acquire(peer);
        permits.pop();
        let replacement = limiter.try_acquire(peer);

        // Then
        assert!(excess.is_none());
        assert!(replacement.is_some());
    }

    #[test]
    fn global_limit_is_rejected_and_drop_releases_capacity() {
        // Given
        let limiter = EchoLimiter::default();
        let mut permits = (0..ECHO_GLOBAL_LIMIT)
            .map(|_| {
                let peer = EndpointSecret::generate().endpoint_id();
                limiter.try_acquire(peer).expect("within global limit")
            })
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), ECHO_GLOBAL_LIMIT);
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
