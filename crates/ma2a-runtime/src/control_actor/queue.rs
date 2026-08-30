use tokio::sync::oneshot;

use crate::{control_sync::ControlRoundScope, error::RuntimeError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ControlRoundId(u64);

#[derive(Clone, Debug)]
pub(crate) struct ScheduledControlRound {
    id: ControlRoundId,
    scope: ControlRoundScope,
    rotation: usize,
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
}

struct ControlWaiter {
    round_id: ControlRoundId,
    reply: oneshot::Sender<Result<u64, RuntimeError>>,
}

#[derive(Default)]
pub(crate) struct ControlRoundQueue {
    next_id: u64,
    rotation: usize,
    active: Option<ControlRoundId>,
    pending: Option<ControlRoundScope>,
    waiters: Vec<ControlWaiter>,
}

impl ControlRoundQueue {
    pub(crate) fn request(
        &mut self,
        scope: ControlRoundScope,
        reply: Option<oneshot::Sender<Result<u64, RuntimeError>>>,
    ) -> Option<ScheduledControlRound> {
        if self.active.is_some() {
            let round_id = ControlRoundId(self.next_id);
            match &mut self.pending {
                Some(pending) => pending.merge(scope),
                None => self.pending = Some(scope),
            }
            if let Some(reply) = reply {
                self.waiters.push(ControlWaiter { round_id, reply });
            }
            return None;
        }
        Some(self.launch(scope, reply))
    }

    pub(crate) const fn active_id(&self) -> Option<ControlRoundId> {
        self.active
    }

    pub(crate) fn complete(
        &mut self,
        round_id: ControlRoundId,
    ) -> Vec<oneshot::Sender<Result<u64, RuntimeError>>> {
        if self.active == Some(round_id) {
            self.active = None;
        }
        let mut completed = Vec::new();
        let mut pending = Vec::new();
        for waiter in std::mem::take(&mut self.waiters) {
            if waiter.round_id == round_id {
                completed.push(waiter.reply);
            } else {
                pending.push(waiter);
            }
        }
        self.waiters = pending;
        completed
    }

    pub(crate) fn take_pending(&mut self) -> Option<ScheduledControlRound> {
        self.pending.take().map(|scope| self.launch(scope, None))
    }

    fn launch(
        &mut self,
        scope: ControlRoundScope,
        reply: Option<oneshot::Sender<Result<u64, RuntimeError>>>,
    ) -> ScheduledControlRound {
        let scheduled = ScheduledControlRound {
            id: ControlRoundId(self.next_id),
            scope,
            rotation: self.rotation,
        };
        self.next_id = self.next_id.wrapping_add(1);
        self.rotation = self.rotation.wrapping_add(1);
        self.active = Some(scheduled.id);
        if let Some(reply) = reply {
            self.waiters.push(ControlWaiter {
                round_id: scheduled.id,
                reply,
            });
        }
        scheduled
    }
}
