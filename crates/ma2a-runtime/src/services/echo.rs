use std::time::Instant;

use ma2a_core::{
    AuthorizationEndpoints, AuthorizationRequest, EchoError, EchoRequest, EchoResponse,
    EchoResultClass, EndpointId, RemoteOperation,
};
use ma2a_store::{ControlSpaceState, Repository};

use crate::EchoAuditRecord;

pub(crate) struct EchoDispatchInput {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) peer_endpoint_id: EndpointId,
    pub(crate) request: Vec<u8>,
    pub(crate) metrics: ma2a_net::EchoMetrics,
}

pub(crate) struct EchoDispatchOutcome {
    pub(crate) response: Result<ma2a_net::EchoServiceResponse, EchoError>,
    pub(crate) audit: Option<EchoAuditRecord>,
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

pub(crate) fn dispatch(input: &EchoDispatchInput) -> EchoDispatchOutcome {
    let started = Instant::now();
    input.metrics.record_decoded_request();
    let Ok(request) = EchoRequest::decode(&input.request) else {
        return EchoDispatchOutcome {
            response: Err(EchoError::InvalidInput),
            audit: None,
        };
    };
    if request.target_endpoint_id() != input.local_endpoint_id {
        let audit = EchoAuditRecord::new(
            request.request_id(),
            input.peer_endpoint_id,
            EchoResultClass::InvalidInput,
            request.payload().len(),
            bounded_duration(started.elapsed().as_millis()),
        )
        .ok();
        return EchoDispatchOutcome {
            response: Err(EchoError::InvalidInput),
            audit,
        };
    }
    let duration_ms = bounded_duration(started.elapsed().as_millis());
    let response = EchoResponse::new(
        request.request_id(),
        input.local_endpoint_id,
        request.payload(),
        duration_ms,
    )
    .and_then(|response| response.encode())
    .and_then(|body| ma2a_net::EchoServiceResponse::new(body, duration_ms));
    let audit = EchoAuditRecord::new(
        request.request_id(),
        input.peer_endpoint_id,
        response
            .as_ref()
            .map_or_else(|error| error.result_class(), |_| EchoResultClass::Succeeded),
        request.payload().len(),
        duration_ms,
    )
    .ok();
    EchoDispatchOutcome { response, audit }
}

fn bounded_duration(duration_ms: u128) -> u16 {
    u16::try_from(duration_ms.min(u128::from(ma2a_core::MAX_ECHO_DURATION_MS)))
        .map_or(ma2a_core::MAX_ECHO_DURATION_MS, |bounded| bounded)
}

#[cfg(test)]
mod tests {
    use ma2a_core::{EchoError, EchoRequest, EchoResultClass, RequestId};
    use ma2a_net::{EchoMetrics, EndpointSecret};

    use super::{EchoDispatchInput, dispatch};

    #[test]
    fn target_mismatch_is_audited_as_invalid_input() {
        // Given
        let local = EndpointSecret::generate().endpoint_id();
        let peer = EndpointSecret::generate().endpoint_id();
        let other_target = EndpointSecret::generate().endpoint_id();
        let request_id = RequestId::try_from([0x61; 16].as_slice()).expect("valid request id");
        let request = EchoRequest::new(request_id, other_target, b"wrong target")
            .and_then(|request| request.encode())
            .expect("valid request");
        let input = EchoDispatchInput {
            local_endpoint_id: local,
            peer_endpoint_id: peer,
            request,
            metrics: EchoMetrics::default(),
        };

        // When
        let result = dispatch(&input);

        // Then
        assert!(matches!(result.response, Err(EchoError::InvalidInput)));
        let record = result.audit.expect("one audit record");
        assert_eq!(record.request_id(), request_id);
        assert_eq!(record.peer_endpoint_id(), peer);
        assert_eq!(record.result_class(), EchoResultClass::InvalidInput);
        assert_eq!(record.byte_count(), 12);
    }
}
