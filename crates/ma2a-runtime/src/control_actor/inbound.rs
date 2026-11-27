use ma2a_core::EndpointId;
use ma2a_net::{ControlAuthorizedCall, ControlCall, ControlRejection, ControlResponder};

use crate::{
    actor::Actor,
    control_sync::{ControlChanges, ControlExchangeInput, ControlFailure, ControlRespondOutcome},
};

#[cfg(test)]
mod view_sequence_tests;

#[cfg(test)]
struct InboundControlGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
fn inbound_gates()
-> &'static std::sync::Mutex<std::collections::BTreeMap<EndpointId, InboundControlGate>> {
    static GATES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<EndpointId, InboundControlGate>>,
    > = std::sync::OnceLock::new();
    GATES.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

#[cfg(test)]
pub(crate) fn arm_inbound_control_gate(
    endpoint_id: EndpointId,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    inbound_gates()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            endpoint_id,
            InboundControlGate {
                entered: entered_tx,
                release: release_rx,
            },
        );
    (entered_rx, release_tx)
}

pub(crate) enum InboundControlCompletion {
    NoResponse,
    Response {
        remote_endpoint_id: EndpointId,
        membership_generation: u64,
        responder: ControlResponder,
        result: Result<ControlRespondOutcome, ControlFailure>,
    },
}

struct InboundControlInput {
    store: crate::store::StoreClient,
    call: ControlAuthorizedCall,
    clock: std::sync::Arc<dyn crate::RuntimeClock>,
    local_endpoint_id: EndpointId,
    remote_endpoint_id: EndpointId,
    membership_generation: u64,
}

impl Actor {
    pub(crate) async fn handle_control_call(
        &mut self,
        call: ControlCall,
    ) -> Result<(), crate::RuntimeError> {
        let peer = call.remote_endpoint_id();
        if let Err(error) = self
            .store
            .authorize_control(self.state.endpoint_id, peer)
            .await
        {
            call.reject(error.rejection());
            return Self::control_failure(error);
        }
        let Some(call) = call.authorize() else {
            return Ok(());
        };
        self.retain_control_completion(
            ControlChanges::default(),
            std::collections::BTreeSet::new(),
        );
        let membership_generation = self.synchronized_control_peers.generation();
        self.control_tasks.spawn(run_inbound(InboundControlInput {
            store: self.store.clone(),
            call,
            clock: std::sync::Arc::clone(&self.clock),
            local_endpoint_id: self.state.endpoint_id,
            remote_endpoint_id: peer,
            membership_generation,
        }));
        Ok(())
    }

    pub(crate) async fn finish_control_call(
        &mut self,
        completion: InboundControlCompletion,
    ) -> Result<(), crate::RuntimeError> {
        self.retain_control_completion(
            ControlChanges::default(),
            std::collections::BTreeSet::new(),
        );
        let InboundControlCompletion::Response {
            remote_endpoint_id,
            membership_generation,
            responder,
            result,
        } = completion
        else {
            return Ok(());
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.retain_control_completion(
                    ControlChanges::default(),
                    std::collections::BTreeSet::new(),
                );
                responder.respond(Err(error.rejection()));
                return Self::control_failure(error);
            }
        };
        let peers = if membership_generation == self.synchronized_control_peers.generation() {
            [remote_endpoint_id].into()
        } else {
            std::collections::BTreeSet::new()
        };
        self.state.revision = self.state.revision.max(outcome.revision);
        self.retain_control_completion(outcome.changes, peers);
        match self.reconcile_control_completion().await {
            Ok(()) => responder.respond(Ok(outcome.response)),
            Err(error) => {
                responder.respond(Err(ControlRejection::Unavailable));
                Self::absorb_background("inbound control projection", Err(error))?;
            }
        }
        Ok(())
    }
}

async fn run_inbound(input: InboundControlInput) -> InboundControlCompletion {
    let Ok((request, responder)) = input.call.request().await else {
        return InboundControlCompletion::NoResponse;
    };
    let now_ms = match input.clock.now_ms().and_then(|value| {
        u64::try_from(value)
            .map_err(|_| crate::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))
    }) {
        Ok(now_ms) => now_ms,
        Err(error) => {
            return InboundControlCompletion::Response {
                remote_endpoint_id: input.remote_endpoint_id,
                membership_generation: input.membership_generation,
                responder,
                result: Err(error.into()),
            };
        }
    };
    let result = input
        .store
        .respond_control(ControlExchangeInput {
            local_endpoint_id: input.local_endpoint_id,
            remote_endpoint_id: input.remote_endpoint_id,
            payload: request,
            now_ms,
        })
        .await;
    #[cfg(test)]
    let pending_gate = inbound_gates()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&input.local_endpoint_id);
    #[cfg(test)]
    if let Some(gate) = pending_gate {
        let _sent = gate.entered.send(());
        let _released = gate.release.await;
    }
    InboundControlCompletion::Response {
        remote_endpoint_id: input.remote_endpoint_id,
        membership_generation: input.membership_generation,
        responder,
        result,
    }
}
