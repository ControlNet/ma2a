use ma2a_core::EndpointId;
use ma2a_net::{ControlAuthorizedCall, ControlCall, ControlRejection, ControlResponder};

use crate::{
    actor::Actor,
    control_sync::{ControlChanges, ControlExchangeInput, ControlFailure, ControlRespondOutcome},
};

pub(crate) enum InboundControlCompletion {
    NoResponse,
    Response {
        remote_endpoint_id: EndpointId,
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
        self.control_tasks.spawn(run_inbound(InboundControlInput {
            store: self.store.clone(),
            call,
            clock: std::sync::Arc::clone(&self.clock),
            local_endpoint_id: self.state.endpoint_id,
            remote_endpoint_id: peer,
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
        self.state.revision = self.state.revision.max(outcome.revision);
        self.retain_control_completion(outcome.changes, [remote_endpoint_id].into());
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
    InboundControlCompletion::Response {
        remote_endpoint_id: input.remote_endpoint_id,
        responder,
        result,
    }
}
