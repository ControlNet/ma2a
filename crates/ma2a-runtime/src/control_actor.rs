use std::{collections::BTreeSet, time::Duration};

use ma2a_core::EndpointId;

use crate::{
    actor::Actor,
    control_sync::{
        ControlChanges, ControlFailure, ControlRoundOutcome, ControlRoundRequest,
        ControlRoundRunner, ControlRoundTrigger, install_lookup,
    },
    error::{RuntimeError, RuntimeErrorKind},
};

pub(crate) mod completion;
pub(crate) mod inbound;
mod queue;
pub(crate) use queue::{ControlRoundQueue, ScheduledControlRound};

#[cfg(test)]
mod scheduling_tests;
#[cfg(test)]
mod source_tests;

const CONTROL_PERIOD_BASE_SECS: u64 = 60;
const CONTROL_PERIOD_JITTER_SECS: u64 = 30;

pub(crate) fn control_period(endpoint_id: EndpointId) -> Duration {
    let jitter = endpoint_id
        .as_bytes()
        .iter()
        .take(8)
        .fold(0_u64, |value, byte| value.rotate_left(5) ^ u64::from(*byte))
        % CONTROL_PERIOD_JITTER_SECS;
    Duration::from_secs(CONTROL_PERIOD_BASE_SECS + jitter)
}

impl Actor {
    pub(crate) fn schedule_control_round(
        &mut self,
        trigger: ControlRoundTrigger,
        reply: Option<tokio::sync::oneshot::Sender<Result<u64, RuntimeError>>>,
    ) {
        #[cfg(test)]
        self.control_schedule_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(trigger.clone());
        let Some(scheduled) = self.control_queue.request(trigger.scope(), reply) else {
            return;
        };
        self.spawn_control_round(scheduled);
    }

    fn spawn_control_round(&mut self, scheduled: ScheduledControlRound) {
        let now_ms = match self.clock.now_ms().and_then(|value| {
            u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
        }) {
            Ok(now) => now,
            Err(error) => {
                self.maintenance.fatal = Some(error);
                self.complete_control_waiters(scheduled.id(), false, &BTreeSet::new());
                return;
            }
        };
        let store = self.store.clone();
        let client = self.endpoint.control_client();
        let endpoint_id = self.state.endpoint_id;
        let task = scheduled;
        self.control_rounds.spawn(async move {
            let result = ControlRoundRunner::new(store, client)
                .run(ControlRoundRequest {
                    local_endpoint_id: endpoint_id,
                    rotation: task.rotation(),
                    now_ms,
                    scope: task.scope().clone(),
                })
                .await;
            (task, result)
        });
    }

    pub(crate) async fn finish_control_round(
        &mut self,
        result: Result<
            (
                ScheduledControlRound,
                Result<Option<ControlRoundOutcome>, ControlFailure>,
            ),
            tokio::task::JoinError,
        >,
    ) -> Result<(), RuntimeError> {
        let (scheduled, outcome) = result.map_err(RuntimeError::from)?;
        let round_id = scheduled.id();
        let mut failure = None;
        let peers = match outcome {
            Ok(Some(outcome)) => {
                self.state.revision = self.state.revision.max(outcome.revision);
                self.retain_control_completion(outcome.changes, outcome.synchronized_peers.clone());
                outcome.synchronized_peers
            }
            Ok(None) => BTreeSet::new(),
            Err(error) => {
                self.retain_control_completion(ControlChanges::default(), BTreeSet::new());
                failure = Some(error);
                BTreeSet::new()
            }
        };
        let reconciled = self.reconcile_control_completion().await;
        let succeeded = failure.is_none() && reconciled.is_ok();
        self.record_control_round(succeeded, &peers);
        self.complete_control_waiters(round_id, succeeded, &peers);
        Self::absorb_background("control projection", reconciled)?;
        if let Some(error) = failure {
            Self::control_failure(error)?;
        }
        Ok(())
    }

    /// Retains a bounded in-memory round history. It is a diagnostic, never a log:
    /// it is not persisted and does not survive a restart.
    fn record_control_round(&mut self, succeeded: bool, synchronized_peers: &BTreeSet<EndpointId>) {
        let at_ms = match self.clock.now_ms().and_then(|now| {
            u64::try_from(now).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
        }) {
            Ok(now) => now,
            Err(error) => {
                self.maintenance.fatal = Some(error);
                return;
            }
        };
        let outcome = if !succeeded {
            "failed"
        } else if synchronized_peers.is_empty() {
            "empty"
        } else {
            "succeeded"
        };
        let peer_count = u32::try_from(synchronized_peers.len()).unwrap_or(u32::MAX);
        let Ok(record) = crate::api::ControlRoundView::new(at_ms, peer_count, outcome) else {
            return;
        };
        if self.control_round_history.len() >= crate::api::MAX_RETAINED_CONTROL_ROUNDS {
            self.control_round_history.pop_front();
        }
        self.control_round_history.push_back(record);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "waiter completion requires the complete round outcome"
    )]
    fn complete_control_waiters(
        &mut self,
        round_id: queue::ControlRoundId,
        succeeded: bool,
        synchronized_peers: &BTreeSet<EndpointId>,
    ) {
        for waiter in self.control_queue.complete(round_id, succeeded) {
            let waiter_succeeded = succeeded
                && waiter
                    .peer()
                    .is_none_or(|peer| synchronized_peers.contains(&peer));
            let result = if waiter_succeeded {
                Ok(self.state.revision)
            } else {
                Err(RuntimeError::new(RuntimeErrorKind::Control))
            };
            waiter.send(result);
        }
        if let Some(scheduled) = self.control_queue.take_pending() {
            self.spawn_control_round(scheduled);
        }
    }

    fn schedule_control_changes(&mut self, changes: ControlChanges) {
        for trigger in changes.triggers() {
            self.schedule_control_round(trigger, None);
        }
    }

    pub(crate) async fn refresh_control_lookup(&self) -> Result<(), RuntimeError> {
        let now_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let state = self
            .store
            .load_control_lookup(self.state.endpoint_id, now_ms)
            .await?;
        install_lookup(&self.lookup, state)
    }
}
