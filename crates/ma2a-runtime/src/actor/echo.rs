use ma2a_core::{EchoError, EchoRequest, EchoResponse, EchoResultClass, EndpointId, RequestId};
use ma2a_net::EchoCall;
use tokio::sync::oneshot;

use super::Actor;
use crate::{EchoAuditRecord, services::echo::EchoDispatchInput};

impl Actor {
    pub(super) async fn handle_echo_call(&mut self, call: EchoCall) {
        let peer = call.remote_endpoint_id();
        if self
            .store
            .authorize_echo(self.state.endpoint_id, peer)
            .await
            .is_err()
        {
            call.deny();
            return;
        }
        let Some(call) = call.authorize() else {
            return;
        };
        let input = InboundEchoInput {
            call,
            local_endpoint_id: self.state.endpoint_id,
            peer_endpoint_id: peer,
            metrics: self.echo_metrics.clone(),
            audit: self.echo_audit.clone(),
        };
        self.echo_tasks.spawn(run_inbound(input));
    }

    pub(super) fn spawn_outbound_echo(
        &mut self,
        request: OutboundEchoRequest,
        reply: oneshot::Sender<Result<EchoResponse<'static>, EchoError>>,
    ) {
        let OutboundEchoRequest {
            request_id,
            target,
            payload,
        } = request;
        let input = OutboundEchoInput {
            request_id,
            target,
            payload,
            local_endpoint_id: self.state.endpoint_id,
            client: self.endpoint.echo_client(),
            audit: self.echo_audit.clone(),
        };
        self.echo_tasks.spawn(async move {
            let _unsent = reply.send(run_outbound(input).await);
        });
    }
}

struct InboundEchoInput {
    call: ma2a_net::EchoAuthorizedCall,
    local_endpoint_id: EndpointId,
    peer_endpoint_id: EndpointId,
    metrics: ma2a_net::EchoMetrics,
    audit: crate::echo_audit::EchoAuditLog,
}

async fn run_inbound(input: InboundEchoInput) {
    let Ok((request, responder)) = input.call.request().await else {
        return;
    };
    responder.respond(crate::services::dispatch(
        ma2a_core::ServiceKind::Echo,
        &EchoDispatchInput {
            local_endpoint_id: input.local_endpoint_id,
            peer_endpoint_id: input.peer_endpoint_id,
            request,
            metrics: input.metrics,
            audit: input.audit,
        },
    ));
}

struct OutboundEchoInput {
    request_id: RequestId,
    target: EndpointId,
    payload: Vec<u8>,
    local_endpoint_id: EndpointId,
    client: ma2a_net::EchoClient,
    audit: crate::echo_audit::EchoAuditLog,
}

pub(super) struct OutboundEchoRequest {
    pub(super) request_id: RequestId,
    pub(super) target: EndpointId,
    pub(super) payload: Vec<u8>,
}

async fn run_outbound(input: OutboundEchoInput) -> Result<EchoResponse<'static>, EchoError> {
    let request = EchoRequest::new(input.request_id, input.target, &input.payload)?;
    let response = input
        .client
        .exchange(input.target, &request.encode()?)
        .await?;
    if response.request_id() != input.request_id {
        return Err(EchoError::InvalidInput);
    }
    input.audit.record(EchoAuditRecord::new(
        input.request_id,
        input.target,
        EchoResultClass::Succeeded,
        input.payload.len(),
        response.duration_ms(),
    )?);
    let _local_endpoint_id = input.local_endpoint_id;
    Ok(response)
}
