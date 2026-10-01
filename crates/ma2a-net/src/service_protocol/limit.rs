use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::{EndpointId, ServiceKind};

/// Per-peer and Runtime-wide concurrent stream bounds of one service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceLimits {
    per_peer: usize,
    global: usize,
}

impl ServiceLimits {
    /// Returns the fixed Phase 2 bounds of a streaming service.
    pub const fn of(kind: ServiceKind) -> Self {
        let (per_peer, global) = match kind {
            ServiceKind::Echo => (16, 128),
            ServiceKind::Files | ServiceKind::Process => (16, 64),
            ServiceKind::Transfer | ServiceKind::Shell => (4, 16),
            ServiceKind::Exec => (8, 32),
            ServiceKind::Proxy => (64, 256),
        };
        Self { per_peer, global }
    }

    /// Returns the per-peer bound.
    pub const fn per_peer(self) -> usize {
        self.per_peer
    }

    /// Returns the Runtime-wide bound.
    pub const fn global(self) -> usize {
        self.global
    }
}

#[derive(Clone, Debug)]
pub(super) struct ServiceLimiter {
    limits: ServiceLimits,
    state: Arc<std::sync::Mutex<LimitState>>,
}

#[derive(Debug, Default)]
struct LimitState {
    total: usize,
    peers: BTreeMap<EndpointId, usize>,
}

/// RAII capacity held for the whole life of one admitted stream.
#[derive(Debug)]
pub struct ServicePermit {
    limiter: ServiceLimiter,
    peer: EndpointId,
}

impl ServiceLimiter {
    pub(super) fn new(limits: ServiceLimits) -> Self {
        Self {
            limits,
            state: Arc::default(),
        }
    }

    pub(super) fn try_acquire(&self, peer: EndpointId) -> Option<ServicePermit> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or_default();
        if state.total >= self.limits.global || peer_count >= self.limits.per_peer {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer, peer_count + 1);
        drop(state);
        Some(ServicePermit {
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

impl Drop for ServicePermit {
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
    use ma2a_core::ServiceKind;

    use super::{ServiceLimiter, ServiceLimits};
    use crate::EndpointSecret;

    #[test]
    fn per_peer_and_global_bounds_release_on_drop() {
        let limits = ServiceLimits::of(ServiceKind::Shell);
        let limiter = ServiceLimiter::new(limits);
        let peer = EndpointSecret::generate().endpoint_id();
        let mut held = (0..limits.per_peer())
            .map(|_| limiter.try_acquire(peer).expect("within bound"))
            .collect::<Vec<_>>();
        assert!(limiter.try_acquire(peer).is_none());
        held.pop();
        held.push(limiter.try_acquire(peer).expect("released capacity"));
        let others = (limits.per_peer()..limits.global())
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
