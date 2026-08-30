use std::time::Duration;

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
        if self.state.memberships.is_empty() {
            if let Some(reply) = reply {
                let _unsent = reply.send(Ok(self.state.revision));
            }
            return;
        }
        let Some(scheduled) = self.control_queue.request(trigger.scope(), reply) else {
            return;
        };
        self.spawn_control_round(scheduled);
    }

    fn spawn_control_round(&mut self, scheduled: ScheduledControlRound) {
        let Ok(now_ms) = self.clock.now_ms().and_then(|value| {
            u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
        }) else {
            self.complete_control_waiters(scheduled.id(), false);
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
                        self.schedule_control_changes(outcome.changes);
                        call.respond(Ok(response));
                    }
                    Err(rejection) => call.respond(Err(rejection)),
                }
            }
            Err(rejection) => call.respond(Err(rejection)),
        }
    }

    pub(crate) fn finish_control_round(
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
            self.complete_control_waiters(round_id, false);
            return;
        };
        let round_id = scheduled.id();
        let succeeded = match outcome {
            Ok(Some(outcome)) => {
                let changes = outcome.changes;
                if self.state.memberships != outcome.memberships {
                    self.synchronized_control_peers.clear();
                }
                self.state.revision = self.state.revision.max(outcome.revision);
                self.state.memberships = outcome.memberships;
                self.synchronized_control_peers
                    .extend(outcome.synchronized_peers);
                self.endpoint
                    .set_control_enabled(!self.state.memberships.is_empty());
                self.schedule_control_changes(changes);
                true
            }
            Ok(None) => true,
            Err(_) => false,
        };
        self.complete_control_waiters(round_id, succeeded);
    }

    fn complete_control_waiters(&mut self, round_id: queue::ControlRoundId, succeeded: bool) {
        for waiter in self.control_queue.complete(round_id) {
            let result = if succeeded {
                Ok(self.state.revision)
            } else {
                Err(RuntimeError::new(RuntimeErrorKind::Control))
            };
            let _unsent = waiter.send(result);
        }
        if let Some(scheduled) = self.control_queue.take_pending() {
            self.spawn_control_round(scheduled);
        }
    }

    fn schedule_control_changes(&mut self, changes: ControlChanges) {
        if changes.manifest {
            self.schedule_control_round(ControlRoundTrigger::ManifestAdvanced, None);
        }
        if changes.address {
            self.schedule_control_round(ControlRoundTrigger::AddressAdvanced, None);
        }
        if changes.relay {
            self.schedule_control_round(ControlRoundTrigger::RelayAdvanced, None);
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
