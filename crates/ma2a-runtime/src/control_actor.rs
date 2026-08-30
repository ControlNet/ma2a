use std::{collections::BTreeSet, time::Duration};

use ma2a_core::EndpointId;
use ma2a_net::ControlCall;

use crate::{
    actor::Actor,
    control_sync::{
        ControlChanges, ControlExchangeInput, ControlRoundOutcome, ControlRoundRequest,
        ControlRoundRunner, ControlRoundTrigger, install_lookup,
    },
    error::{RuntimeError, RuntimeErrorKind},
};

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
        let Ok(now_ms) = self.clock.now_ms().and_then(|value| {
            u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
        }) else {
            self.complete_control_waiters(scheduled.id(), false, &BTreeSet::new());
            return;
        };
        let store = self.store.clone();
        let client = self.endpoint.control_client();
        let lookup = self.lookup.clone();
        let endpoint_id = self.state.endpoint_id;
        let task = scheduled;
        self.control_rounds.spawn(async move {
            let result = ControlRoundRunner::new(store, client, lookup)
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

    pub(crate) async fn handle_control_call(&mut self, call: ControlCall) {
        let result = self
            .clock
            .now_ms()
            .ok()
            .and_then(|value| u64::try_from(value).ok())
            .map_or(Err(ma2a_net::ControlRejection::Unavailable), |now_ms| {
                Ok((call.remote_endpoint_id(), call.request().to_vec(), now_ms))
            });
        match result {
            Ok((remote_endpoint_id, request, now_ms)) => {
                let response = self
                    .store
                    .respond_control(ControlExchangeInput {
                        local_endpoint_id: self.state.endpoint_id,
                        remote_endpoint_id,
                        payload: request,
                        now_ms,
                    })
                    .await;
                match response {
                    Ok(outcome) => {
                        let response = outcome.response;
                        if install_lookup(&self.lookup, outcome.lookup).is_err() {
                            call.respond(Err(ma2a_net::ControlRejection::Unavailable));
                            return;
                        }
                        self.state.revision = self.state.revision.max(outcome.revision);
                        self.state.memberships = outcome.memberships;
                        self.synchronized_control_peers.insert(remote_endpoint_id);
                        self.endpoint
                            .set_control_enabled(!self.state.memberships.is_empty());
                        if self.refresh_relay_candidates().await.is_err() {
                            call.respond(Err(ma2a_net::ControlRejection::Unavailable));
                            return;
                        }
                        self.schedule_control_changes(outcome.changes);
                        call.respond(Ok(response));
                    }
                    Err(rejection) => call.respond(Err(rejection)),
                }
            }
            Err(rejection) => call.respond(Err(rejection)),
        }
    }

    pub(crate) async fn finish_control_round(
        &mut self,
        result: Result<
            (
                ScheduledControlRound,
                Result<Option<ControlRoundOutcome>, RuntimeError>,
            ),
            tokio::task::JoinError,
        >,
    ) {
        let Ok((scheduled, outcome)) = result else {
            let Some(round_id) = self.control_queue.active_id() else {
                return;
            };
            self.complete_control_waiters(round_id, false, &BTreeSet::new());
            return;
        };
        let round_id = scheduled.id();
        let (succeeded, synchronized_peers) = match outcome {
            Ok(Some(outcome)) => {
                let changes = outcome.changes;
                let synchronized_peers = outcome.synchronized_peers;
                if self.state.memberships != outcome.memberships {
                    self.synchronized_control_peers.clear();
                }
                self.state.revision = self.state.revision.max(outcome.revision);
                self.state.memberships = outcome.memberships;
                self.synchronized_control_peers
                    .extend(synchronized_peers.iter().copied());
                self.endpoint
                    .set_control_enabled(!self.state.memberships.is_empty());
                let succeeded = self.refresh_relay_candidates().await.is_ok();
                if succeeded {
                    self.schedule_control_changes(changes);
                }
                (succeeded, synchronized_peers)
            }
            Ok(None) => (true, BTreeSet::new()),
            Err(_) => (false, BTreeSet::new()),
        };
        self.complete_control_waiters(round_id, succeeded, &synchronized_peers);
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
