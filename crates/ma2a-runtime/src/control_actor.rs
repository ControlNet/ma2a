use std::time::Duration;

use ma2a_core::EndpointId;
use ma2a_net::ControlCall;

use crate::{
    actor::Actor,
    control_sync::{
        ControlExchangeInput, ControlRoundOutcome, ControlRoundRequest, ControlRoundRunner,
        install_lookup,
    },
    error::{RuntimeError, RuntimeErrorKind},
};

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
    pub(crate) fn schedule_control_round(&mut self) {
        if self.state.memberships.is_empty() {
            return;
        }
        if !self.control_rounds.is_empty() {
            self.control_pending = true;
            return;
        }
        let Ok(now_ms) = self.clock.now_ms().and_then(|value| {
            u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
        }) else {
            return;
        };
        let store = self.store.clone();
        let client = self.endpoint.control_client();
        let lookup = self.lookup.clone();
        let endpoint_id = self.state.endpoint_id;
        let rotation = self.control_rotation;
        self.control_rotation = self.control_rotation.wrapping_add(1);
        self.control_rounds.spawn(async move {
            ControlRoundRunner::new(store, client, lookup)
                .run(ControlRoundRequest {
                    local_endpoint_id: endpoint_id,
                    rotation,
                    now_ms,
                })
                .await
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
        result: Result<Result<Option<ControlRoundOutcome>, RuntimeError>, tokio::task::JoinError>,
    ) {
        let succeeded = match result {
            Ok(Ok(Some(outcome))) => {
                if self.state.memberships != outcome.memberships {
                    self.synchronized_control_peers.clear();
                }
                self.state.revision = self.state.revision.max(outcome.revision);
                self.state.memberships = outcome.memberships;
                self.synchronized_control_peers
                    .extend(outcome.synchronized_peers);
                self.endpoint
                    .set_control_enabled(!self.state.memberships.is_empty());
                true
            }
            Ok(Ok(None)) => true,
            Ok(Err(_)) | Err(_) => false,
        };
        if !succeeded && self.control_pending {
            self.control_pending = false;
            self.schedule_control_round();
            return;
        }
        for waiter in self.control_waiters.drain(..) {
            let result = if succeeded {
                Ok(self.state.revision)
            } else {
                Err(RuntimeError::new(RuntimeErrorKind::Control))
            };
            let _unsent = waiter.send(result);
        }
        if self.control_pending {
            self.control_pending = false;
            self.schedule_control_round();
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
