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
pub(crate) use queue::{ControlRoundQueue, ScheduledControlRound, SynchronizedControlPeers};

#[cfg(test)]
mod fairness_tests;
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
                self.complete_control_waiters(scheduled.id(), false, &BTreeSet::new(), None);
                return;
            }
        };
        let store = self.store.clone();
        let client = self.endpoint.control_client();
        let clock = std::sync::Arc::clone(&self.clock);
        let endpoint_id = self.state.endpoint_id;
        let task = scheduled;
        let lookup_ready = self
            .maintenance
            .control_completion
            .as_ref()
            .and_then(completion::PendingControl::lookup_waiter);
        #[cfg(test)]
        let gate = std::sync::Arc::clone(&self.maintenance.control_round_gate);
        self.control_rounds.spawn(async move {
            let mut runner = ControlRoundRunner::new(store, client, clock);
            if let Some(lookup_ready) = lookup_ready {
                runner = runner.with_lookup_ready(lookup_ready);
            }
            let result = runner
                .run(ControlRoundRequest {
                    local_endpoint_id: endpoint_id,
                    rotation: task.rotation(),
                    now_ms,
                    scope: task.scope().clone(),
                })
                .await;
            #[cfg(test)]
            let pending_gate = gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            #[cfg(test)]
            if let Some(gate) = pending_gate {
                let _sent = gate.entered.send(());
                let _released = gate.release.await;
            }
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
        let current_round =
            scheduled.membership_generation() == self.synchronized_control_peers.generation();
        let before_view = self.view_sequence;
        let mut failure = None;
        let (peers, dialed) = match outcome {
            Ok(Some(outcome)) => {
                if let Some(revision) = outcome.revision {
                    self.state.revision = self.state.revision.max(revision);
                }
                let peers = if current_round {
                    outcome.synchronized_peers
                } else {
                    BTreeSet::new()
                };
                if !outcome.changes.is_empty() || !peers.is_empty() {
                    self.retain_control_completion(outcome.changes, peers.clone());
                }
                if outcome.incomplete {
                    // The round failed before finishing; its committed changes still stay.
                    failure = Some(ma2a_net::ControlRejection::Unavailable.into());
                }
                (peers, Some(outcome.dialed_peers))
            }
            Ok(None) => (BTreeSet::new(), Some(BTreeSet::new())),
            Err(error) => {
                failure = Some(error);
                (BTreeSet::new(), None)
            }
        };
        if let Some(error) = failure {
            self.record_control_round((false, &peers), before_view);
            self.schedule_control_projection();
            self.complete_control_waiters(round_id, false, &peers, dialed.as_ref());
            Self::control_failure(error)?;
            return Ok(());
        }
        let reconciled = self.reconcile_control_completion().await;
        let succeeded = reconciled.is_ok();
        self.record_control_round((succeeded, &peers), before_view);
        self.complete_control_waiters(round_id, succeeded, &peers, dialed.as_ref());
        Self::absorb_background("control projection", reconciled)?;
        Ok(())
    }

    fn schedule_control_projection(&mut self) {
        if self.maintenance.control_completion.is_some()
            && self.maintenance.control_projections.is_empty()
        {
            let store = self.store.clone();
            self.maintenance.control_projections.spawn(async move {
                let _ = store.revision().await;
            });
        }
    }

    /// Retains a bounded in-memory round history. It is a diagnostic, never a log:
    /// it is not persisted and does not survive a restart.
    fn record_control_round(
        &mut self,
        (succeeded, synchronized_peers): (bool, &BTreeSet<EndpointId>),
        before_view: u64,
    ) {
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
        if self.view_sequence == before_view {
            self.view_sequence = self.view_sequence.saturating_add(1);
        }
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
        dialed_peers: Option<&BTreeSet<EndpointId>>,
    ) {
        for waiter in self.control_queue.complete(round_id, succeeded) {
            let result = if succeeded
                && waiter
                    .peer()
                    .is_none_or(|peer| synchronized_peers.contains(&peer))
            {
                Ok(self.state.revision)
            } else if waiter
                .peer()
                .is_some_and(|peer| dialed_peers.is_some_and(|dialed| !dialed.contains(&peer)))
            {
                Err(RuntimeError::new(RuntimeErrorKind::ControlNotDialed))
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
