//! Per-peer and Runtime-wide bounds on enrollment connections being served at once.

use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::EndpointId;

use super::{ENROLLMENT_GLOBAL_LIMIT, ENROLLMENT_PER_PEER_LIMIT};

#[derive(Clone, Debug, Default)]
pub(super) struct EnrollmentLimiter(Arc<std::sync::Mutex<LimitState>>);

#[derive(Debug, Default)]
struct LimitState {
    total: usize,
    peers: BTreeMap<EndpointId, usize>,
}

#[derive(Debug)]
pub(super) struct EnrollmentPermit {
    limiter: EnrollmentLimiter,
    peer: EndpointId,
}

impl EnrollmentLimiter {
    pub(super) fn try_acquire(&self, peer: EndpointId) -> Option<EnrollmentPermit> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or_default();
        if state.total >= ENROLLMENT_GLOBAL_LIMIT || peer_count >= ENROLLMENT_PER_PEER_LIMIT {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer, peer_count + 1);
        drop(state);
        Some(EnrollmentPermit {
            limiter: self.clone(),
            peer,
        })
    }
}

impl Drop for EnrollmentPermit {
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
