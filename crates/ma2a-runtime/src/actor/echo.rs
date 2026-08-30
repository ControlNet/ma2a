use std::time::Instant;

use ma2a_core::{EchoError, EchoRequest, EchoResponse, EchoResultClass, EndpointId, RequestId};
use ma2a_net::EchoCall;
use tokio::sync::oneshot;

use super::Actor;
use crate::{EchoAuditRecord, services::echo::EchoDispatchInput};

impl Actor {
    pub(super) async fn handle_echo_call(&mut self, call: EchoCall) {
        let peer = call.remote_endpoint_id();
        let authorization = self
            .store
            .authorize_echo(self.state.endpoint_id, peer)
            .await;
        if let Err(error) = authorization {
            call.reject(error);
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
    client: ma2a_net::EchoClient,
    audit: crate::echo_audit::EchoAuditLog,
}

struct OutboundAudit<'a> {
    log: &'a crate::echo_audit::EchoAuditLog,
    request_id: RequestId,
    target: EndpointId,
    byte_count: usize,
}

impl OutboundAudit<'_> {
    fn record(
        &self,
        result: &Result<EchoResponse<'static>, EchoError>,
        duration_ms: u16,
    ) -> Result<(), EchoError> {
        self.log.record(EchoAuditRecord::new(
            self.request_id,
            self.target,
            result
                .as_ref()
                .map_or_else(|error| error.result_class(), |_| EchoResultClass::Succeeded),
            self.byte_count,
            duration_ms,
        )?);
        Ok(())
    }
}

pub(super) struct OutboundEchoRequest {
    pub(super) request_id: RequestId,
    pub(super) target: EndpointId,
    pub(super) payload: Vec<u8>,
}

async fn run_outbound(input: OutboundEchoInput) -> Result<EchoResponse<'static>, EchoError> {
    let started = Instant::now();
    let audit = OutboundAudit {
        log: &input.audit,
        request_id: input.request_id,
        target: input.target,
        byte_count: input.payload.len(),
    };
    let result = async {
        let request = EchoRequest::new(input.request_id, input.target, &input.payload)?;
        let response = input
            .client
            .exchange(input.target, &request.encode()?)
            .await?;
        if response.request_id() != input.request_id {
            return Err(EchoError::InvalidInput);
        }
        Ok(response)
    }
    .await;
    let duration_ms = result.as_ref().map_or_else(
        |_| bounded_duration(started.elapsed().as_millis()),
        EchoResponse::duration_ms,
    );
    audit.record(&result, duration_ms)?;
    result
}

fn bounded_duration(duration_ms: u128) -> u16 {
    u16::try_from(duration_ms.min(u128::from(ma2a_core::MAX_ECHO_DURATION_MS)))
        .map_or(ma2a_core::MAX_ECHO_DURATION_MS, |bounded| bounded)
}

#[cfg(test)]
mod tests {
    use ma2a_core::{EchoError, EchoResultClass, RequestId};
    use ma2a_net::EndpointSecret;

    use super::{OutboundAudit, bounded_duration};
    use crate::echo_audit::EchoAuditLog;

    #[test]
    fn outbound_unavailable_result_is_audited_without_payload() {
        // Given
        let audit = EchoAuditLog::default();
        let request_id = RequestId::try_from([0x64; 16].as_slice()).expect("valid request id");
        let target = EndpointSecret::generate().endpoint_id();
        let error = EchoError::Unavailable;

        // When
        OutboundAudit {
            log: &audit,
            request_id,
            target,
            byte_count: 7,
        }
        .record(&Err(error), bounded_duration(4))
        .expect("bounded record");

        // Then
        let records = audit.snapshot();
        assert_eq!(records.len(), 1);
        let record = records.first().expect("one audit record");
        assert_eq!(record.request_id(), request_id);
        assert_eq!(record.peer_endpoint_id(), target);
        assert_eq!(record.result_class(), EchoResultClass::Unavailable);
        assert_eq!(record.byte_count(), 7);
    }
}
