use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_core::EndpointId;
use tokio::sync::oneshot;

use crate::{control_sync::ControlRoundScope, error::RuntimeError};

#[derive(Debug)]
pub(crate) struct SynchronizedControlPeers {
    generation: Arc<AtomicU64>,
    peers: BTreeSet<EndpointId>,
}

impl SynchronizedControlPeers {
    pub(crate) const fn new(generation: Arc<AtomicU64>) -> Self {
        Self {
            generation,
            peers: BTreeSet::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.peers.clear();
    }

    pub(crate) fn contains(&self, peer: &EndpointId) -> bool {
        self.peers.contains(peer)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &EndpointId> {
        self.peers.iter()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) fn extend(&mut self, peers: impl IntoIterator<Item = EndpointId>) {
        self.peers.extend(peers);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ControlRoundId(u64);

#[derive(Clone, Debug)]
pub(crate) struct ScheduledControlRound {
    id: ControlRoundId,
    scope: ControlRoundScope,
    rotation: usize,
    membership_generation: u64,
}

impl ScheduledControlRound {
    pub(crate) const fn id(&self) -> ControlRoundId {
        self.id
    }

    pub(crate) const fn scope(&self) -> &ControlRoundScope {
        &self.scope
    }

    pub(crate) const fn rotation(&self) -> usize {
        self.rotation
    }

    pub(crate) const fn membership_generation(&self) -> u64 {
        self.membership_generation
    }
}

pub(super) struct ControlWaiter {
    round_id: Option<ControlRoundId>,
    peer: Option<ma2a_core::EndpointId>,
    reply: oneshot::Sender<Result<u64, RuntimeError>>,
}

impl ControlWaiter {
    pub(super) const fn peer(&self) -> Option<ma2a_core::EndpointId> {
        self.peer
    }

    pub(super) fn send(self, result: Result<u64, RuntimeError>) {
        let _unsent = self.reply.send(result);
    }
}

pub(crate) struct ControlRoundQueue {
    next_id: u64,
    rotation: usize,
    active: Option<ControlRoundId>,
    pending: Option<ControlRoundScope>,
    waiters: Vec<ControlWaiter>,
    healthy: bool,
    /// Whether the last round launched was targeted; a pending global round then goes next,
    /// so targeted requests arriving during every round cannot starve it.
    last_targeted: bool,
    membership_generation: Arc<AtomicU64>,
}

impl Default for ControlRoundQueue {
    fn default() -> Self {
        Self::with_generation(Arc::new(AtomicU64::new(0)))
    }
}

impl ControlRoundQueue {
    pub(crate) const fn with_generation(membership_generation: Arc<AtomicU64>) -> Self {
        Self {
            next_id: 0,
            rotation: 0,
            active: None,
            pending: None,
            waiters: Vec::new(),
            healthy: true,
            last_targeted: false,
            membership_generation,
        }
    }
    pub(crate) fn request(
        &mut self,
        scope: ControlRoundScope,
        reply: Option<oneshot::Sender<Result<u64, RuntimeError>>>,
    ) -> Option<ScheduledControlRound> {
        self.healthy = false;
        let waiter_peer = scope.waiter_peer();
        if self.active.is_some() {
            match &mut self.pending {
                Some(pending) => pending.merge(scope),
                None => self.pending = Some(scope),
            }
            if let Some(reply) = reply {
                self.waiters.push(ControlWaiter {
                    round_id: None,
                    peer: waiter_peer,
                    reply,
                });
            }
            return None;
        }
        self.pending = Some(scope);
        self.take_pending_with_waiter(reply.map(|reply| ControlWaiter {
            round_id: None,
            peer: waiter_peer,
            reply,
        }))
    }

    #[cfg(test)]
    pub(crate) const fn is_synchronized(&self) -> bool {
        self.healthy && self.active.is_none() && self.pending.is_none()
    }

    pub(super) fn complete(
        &mut self,
        round_id: ControlRoundId,
        succeeded: bool,
    ) -> Vec<ControlWaiter> {
        if self.active == Some(round_id) {
            self.active = None;
            self.healthy = succeeded;
        }
        let mut completed = Vec::new();
        let mut pending = Vec::new();
        for waiter in std::mem::take(&mut self.waiters) {
            if waiter.round_id == Some(round_id) {
                completed.push(waiter);
            } else {
                pending.push(waiter);
            }
        }
        self.waiters = pending;
        completed
    }

    pub(crate) fn take_pending(&mut self) -> Option<ScheduledControlRound> {
        self.take_pending_with_waiter(None)
    }

    fn take_pending_with_waiter(
        &mut self,
        waiter: Option<ControlWaiter>,
    ) -> Option<ScheduledControlRound> {
        let mut pending = self.pending.take()?;
        let scope = pending.take_round(self.last_targeted)?;
        if !pending.is_empty() {
            self.pending = Some(pending);
        }
        Some(self.launch(scope, waiter))
    }

    fn launch(
        &mut self,
        scope: ControlRoundScope,
        waiter: Option<ControlWaiter>,
    ) -> ScheduledControlRound {
        let scheduled = ScheduledControlRound {
            id: ControlRoundId(self.next_id),
            scope,
            rotation: self.rotation,
            membership_generation: self.membership_generation.load(Ordering::Relaxed),
        };
        self.next_id = self.next_id.wrapping_add(1);
        self.rotation = self.rotation.wrapping_add(1);
        self.last_targeted = !scheduled.scope.is_global();
        self.active = Some(scheduled.id);
        for waiter in &mut self.waiters {
            if waiter.round_id.is_none()
                && waiter.peer.map_or_else(
                    || scheduled.scope.is_global(),
                    |peer| scheduled.scope.contains_peer(peer),
                )
            {
                waiter.round_id = Some(scheduled.id);
            }
        }
        if let Some(mut waiter) = waiter {
            waiter.round_id = Some(scheduled.id);
            self.waiters.push(waiter);
        }
        scheduled
    }
}
