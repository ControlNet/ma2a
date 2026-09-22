use ma2a_core::EndpointId;
use ma2a_net::{ControlAuthorizedCall, ControlCall, ControlRejection, ControlResponder};

use crate::{
    actor::Actor,
    control_sync::{ControlExchangeInput, ControlRespondOutcome, install_lookup},
};

pub(crate) enum InboundControlCompletion {
    NoResponse,
    Response {
        remote_endpoint_id: EndpointId,
        responder: ControlResponder,
        result: Result<ControlRespondOutcome, ControlRejection>,
    },
}

struct InboundControlInput {
    store: crate::store::StoreClient,
    call: ControlAuthorizedCall,
    clock: std::sync::Arc<dyn crate::RuntimeClock>,
    local_endpoint_id: EndpointId,
    remote_endpoint_id: EndpointId,
}

impl Actor {
    pub(crate) async fn handle_control_call(&mut self, call: ControlCall) {
        let peer = call.remote_endpoint_id();
        if let Err(rejection) = self
            .store
            .authorize_control(self.state.endpoint_id, peer)
            .await
        {
            call.reject(rejection);
            return;
        }
        let Some(call) = call.authorize() else {
            return;
        };
        self.control_tasks.spawn(run_inbound(InboundControlInput {
            store: self.store.clone(),
            call,
            clock: std::sync::Arc::clone(&self.clock),
            local_endpoint_id: self.state.endpoint_id,
            remote_endpoint_id: peer,
        }));
    }

    pub(crate) async fn finish_control_call(&mut self, completion: InboundControlCompletion) {
        let InboundControlCompletion::Response {
            remote_endpoint_id,
            responder,
            result,
        } = completion
        else {
            return;
        };
        let Ok(outcome) = result else {
            responder.respond(result.map(|outcome| outcome.response));
            return;
        };
        let response = outcome.response;
        if install_lookup(&self.lookup, outcome.lookup).is_err() {
            responder.respond(Err(ControlRejection::Unavailable));
            return;
        }
        let _changed = self.adopt_control_memberships(outcome.revision).await;
        self.synchronized_control_peers.insert(remote_endpoint_id);
        if self.refresh_relay_candidates().await.is_err() {
            responder.respond(Err(ControlRejection::Unavailable));
            return;
        }
        self.schedule_control_changes(outcome.changes);
        responder.respond(Ok(response));
    }
}

async fn run_inbound(input: InboundControlInput) -> InboundControlCompletion {
    let Ok((request, responder)) = input.call.request().await else {
        return InboundControlCompletion::NoResponse;
    };
    let now_ms = input
        .clock
        .now_ms()
        .ok()
        .and_then(|value| u64::try_from(value).ok());
    let Some(now_ms) = now_ms else {
        return InboundControlCompletion::Response {
            remote_endpoint_id: input.remote_endpoint_id,
            responder,
            result: Err(ControlRejection::Unavailable),
        };
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
    InboundControlCompletion::Response {
        remote_endpoint_id: input.remote_endpoint_id,
        responder,
        result,
    }
}
