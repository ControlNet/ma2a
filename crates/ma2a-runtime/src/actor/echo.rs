use std::time::Instant;

use ma2a_core::{EchoError, EchoRequest, EchoResponse, EchoResultClass, EndpointId, RequestId};
use ma2a_net::EchoCall;
use tokio::sync::oneshot;

use super::Actor;
use crate::{EchoAuditRecord, services::echo::EchoDispatchInput};

/// Most outbound Echo calls that wait on their targets at once: as many as the local API serves
/// requests at once, so normally only callers that bypass it, such as an embedding application,
/// meet it.
pub(crate) const MAX_PENDING_OUTBOUND_ECHOES: usize = 32;

/// What an outbound Echo answers: its response at the revision of its audit, or why it has none.
pub(crate) type OutboundEchoResult =
    Result<ma2a_store::Committed<EchoResponse<'static>>, OutboundEchoError>;

/// Why an outbound Echo has no response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutboundEchoError {
    /// Refused before it was admitted: nothing was sent, audited, or committed.
    NotAdmitted(EchoError),
    /// Admitted, and failed.
    Failed(EchoError),
}

impl From<OutboundEchoError> for EchoError {
    fn from(error: OutboundEchoError) -> Self {
        match error {
            OutboundEchoError::NotAdmitted(error) | OutboundEchoError::Failed(error) => error,
        }
    }
}

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
        };
        self.echo_tasks.spawn(run_inbound(input));
    }

    pub(super) fn spawn_outbound_echo(
        &mut self,
        request: OutboundEchoRequest,
        reply: oneshot::Sender<OutboundEchoResult>,
    ) {
        if self.outbound_echoes >= MAX_PENDING_OUTBOUND_ECHOES {
            let refused = OutboundEchoError::NotAdmitted(EchoError::ConcurrencyExceeded);
            let _unsent = reply.send(Err(refused));
            return;
        }
        self.outbound_echoes += 1;
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
        };
        self.echo_tasks
            .spawn(async move { run_outbound(input, reply).await });
    }

    pub(super) async fn finish_echo(&mut self, completion: EchoTaskCompletion) {
        let (audit, response) = match completion {
            EchoTaskCompletion::NoResponse => return,
            EchoTaskCompletion::Inbound {
                audit,
                response,
                finish,
            } => (audit, EchoTaskResponse::Inbound(response, finish)),
            EchoTaskCompletion::Outbound {
                audit,
                result,
                reply,
            } => {
                self.outbound_echoes -= 1;
                (audit, EchoTaskResponse::Outbound(result, reply))
            }
        };
        let revision = match audit {
            Some(record) => match self.store.advance_revision().await {
                Ok(revision) => {
                    self.echo_audit.record(record);
                    self.state.revision = revision;
                    let _receiver_count = self
                        .events
                        .send(crate::state::RuntimeEvent::echo_summary_changed(revision));
                    Ok(revision)
                }
                Err(error) => Err(error),
            },
            None => Ok(self.state.revision),
        };
        match response {
            EchoTaskResponse::Inbound(response, finish) => {
                finish(revision.map_or(Err(EchoError::Unavailable), |_| response));
            }
            EchoTaskResponse::Outbound(result, reply) => {
                let result = revision.map_or(Err(EchoError::Unavailable), |revision| {
                    result.map(|value| ma2a_store::Committed::new(revision, value))
                });
                let _unsent = reply.send(result.map_err(OutboundEchoError::Failed));
            }
        }
    }
}

struct InboundEchoInput {
    call: ma2a_net::EchoAuthorizedCall,
    local_endpoint_id: EndpointId,
    peer_endpoint_id: EndpointId,
    metrics: ma2a_net::EchoMetrics,
}

async fn run_inbound(input: InboundEchoInput) -> EchoTaskCompletion {
    let Ok((request, responder)) = input.call.request().await else {
        return EchoTaskCompletion::NoResponse;
    };
    let outcome = crate::services::dispatch(
        ma2a_core::ServiceKind::Echo,
        &EchoDispatchInput {
            local_endpoint_id: input.local_endpoint_id,
            peer_endpoint_id: input.peer_endpoint_id,
            request,
            metrics: input.metrics,
        },
    );
    EchoTaskCompletion::Inbound {
        audit: outcome.audit,
        response: outcome.response,
        finish: Box::new(move |response| responder.respond(response)),
    }
}

struct OutboundEchoInput {
    request_id: RequestId,
    target: EndpointId,
    payload: Vec<u8>,
    client: ma2a_net::EchoClient,
}

struct OutboundAudit<'a> {
    request_id: RequestId,
    target: EndpointId,
    byte_count: usize,
    result: &'a Result<EchoResponse<'static>, EchoError>,
}

impl OutboundAudit<'_> {
    fn record(&self, duration_ms: u16) -> Result<EchoAuditRecord, EchoError> {
        EchoAuditRecord::new(
            self.request_id,
            self.target,
            self.result
                .as_ref()
                .map_or_else(|error| error.result_class(), |_| EchoResultClass::Succeeded),
            self.byte_count,
            duration_ms,
        )
    }
}

pub(super) enum EchoTaskCompletion {
    NoResponse,
    Inbound {
        audit: Option<EchoAuditRecord>,
        response: Result<ma2a_net::EchoServiceResponse, EchoError>,
        finish: InboundEchoFinish,
    },
    Outbound {
        audit: Option<EchoAuditRecord>,
        result: Result<EchoResponse<'static>, EchoError>,
        reply: oneshot::Sender<OutboundEchoResult>,
    },
}

enum EchoTaskResponse {
    Inbound(
        Result<ma2a_net::EchoServiceResponse, EchoError>,
        InboundEchoFinish,
    ),
    Outbound(
        Result<EchoResponse<'static>, EchoError>,
        oneshot::Sender<OutboundEchoResult>,
    ),
}

type InboundEchoFinish = Box<dyn FnOnce(Result<ma2a_net::EchoServiceResponse, EchoError>) + Send>;

pub(super) struct OutboundEchoRequest {
    pub(super) request_id: RequestId,
    pub(super) target: EndpointId,
    pub(super) payload: Vec<u8>,
}

async fn run_outbound(
    input: OutboundEchoInput,
    reply: oneshot::Sender<OutboundEchoResult>,
) -> EchoTaskCompletion {
    let started = Instant::now();
    let result = async {
        let request = EchoRequest::new(input.request_id, input.target, &input.payload)?;
        let response = input
            .client
            .exchange(input.target, &request.encode()?)
            .await?;
        echoed(input.request_id, &input.payload, response)
    }
    .await;
    let duration_ms = result.as_ref().map_or_else(
        |_| bounded_duration(started.elapsed().as_millis()),
        EchoResponse::duration_ms,
    );
    let audit = OutboundAudit {
        request_id: input.request_id,
        target: input.target,
        byte_count: input.payload.len(),
        result: &result,
    }
    .record(duration_ms)
    .ok();
    EchoTaskCompletion::Outbound {
        audit,
        result,
        reply,
    }
}

/// `response` if it answers exactly the request this Endpoint sent: the same request ID and the
/// same payload, so a caller that sent UTF-8 text always gets that text back.
fn echoed(
    request_id: RequestId,
    payload: &[u8],
    response: EchoResponse<'static>,
) -> Result<EchoResponse<'static>, EchoError> {
    if response.request_id() != request_id || response.payload() != payload {
        return Err(EchoError::InvalidInput);
    }
    Ok(response)
}

fn bounded_duration(duration_ms: u128) -> u16 {
    u16::try_from(duration_ms.min(u128::from(ma2a_core::MAX_ECHO_DURATION_MS)))
        .map_or(ma2a_core::MAX_ECHO_DURATION_MS, |bounded| bounded)
}

#[cfg(test)]
#[path = "echo_test.rs"]
mod tests;
