use std::time::Instant;

use ma2a_core::{
    AuthorizationEndpoints, AuthorizationRequest, EchoError, EchoRequest, EchoResponse,
    EchoResultClass, EndpointId, RemoteOperation,
};
use ma2a_store::{ControlSpaceState, Repository};

use crate::{EchoAuditRecord, echo_audit::EchoAuditLog};

pub(crate) struct EchoDispatchInput {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) peer_endpoint_id: EndpointId,
    pub(crate) request: Vec<u8>,
    pub(crate) metrics: ma2a_net::EchoMetrics,
    pub(crate) audit: EchoAuditLog,
}

pub(crate) fn authorize(
    repository: &Repository,
    local_endpoint_id: EndpointId,
    peer_endpoint_id: EndpointId,
) -> Result<(), EchoError> {
    let shared = repository
        .control_spaces_between(local_endpoint_id, peer_endpoint_id)
        .map_err(|_| EchoError::Unavailable)?;
    let authorizations = shared
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
    crate::authz::authorize_remote(
        &AuthorizationRequest::new(
            AuthorizationEndpoints::new(peer_endpoint_id, local_endpoint_id),
            RemoteOperation::ECHO_CALL,
            None,
        ),
        &authorizations,
    )
    .map(|_| ())
    .map_err(|_| EchoError::Unauthorized)
}

pub(crate) fn dispatch(
    input: &EchoDispatchInput,
) -> Result<ma2a_net::EchoServiceResponse, EchoError> {
    let started = Instant::now();
    input.metrics.record_decoded_request();
    let request = EchoRequest::decode(&input.request)?;
    if request.target_endpoint_id() != input.local_endpoint_id {
        return Err(EchoError::InvalidInput);
    }
    let duration_ms = bounded_duration(started.elapsed().as_millis());
    let response = EchoResponse::new(
        request.request_id(),
        input.local_endpoint_id,
        request.payload(),
        duration_ms,
    )?;
    let body = response.encode()?;
    input.audit.record(EchoAuditRecord::new(
        request.request_id(),
        input.peer_endpoint_id,
        EchoResultClass::Succeeded,
        request.payload().len(),
        duration_ms,
    )?);
    ma2a_net::EchoServiceResponse::new(body, duration_ms)
}

fn bounded_duration(duration_ms: u128) -> u16 {
    u16::try_from(duration_ms.min(u128::from(ma2a_core::MAX_ECHO_DURATION_MS)))
        .map_or(ma2a_core::MAX_ECHO_DURATION_MS, |bounded| bounded)
}
