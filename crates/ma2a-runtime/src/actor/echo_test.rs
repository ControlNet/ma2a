//! Outbound Echo audit records and reply checks.

use ma2a_core::{EchoError, EchoResponse, EchoResultClass, RequestId};
use ma2a_net::EndpointSecret;

use super::{OutboundAudit, bounded_duration, echoed};

#[test]
fn outbound_unavailable_result_is_audited_without_payload() {
    // Given
    let request_id = RequestId::try_from([0x64; 16].as_slice()).expect("valid request id");
    let target = EndpointSecret::generate().endpoint_id();
    let error = EchoError::Unavailable;

    // When
    let record = OutboundAudit {
        request_id,
        target,
        byte_count: 7,
        result: &Err(error),
    }
    .record(bounded_duration(4))
    .expect("bounded record");
    assert_eq!(record.request_id(), request_id);
    assert_eq!(record.peer_endpoint_id(), target);
    assert_eq!(record.result_class(), EchoResultClass::Unavailable);
    assert_eq!(record.byte_count(), 7);
}

#[test]
fn a_reply_that_does_not_echo_the_sent_payload_is_refused() {
    // Given a reply to this request that carries other bytes than were sent.
    let request_id = RequestId::try_from([0x65; 16].as_slice()).expect("valid request id");
    let responder = EndpointSecret::generate().endpoint_id();
    let reply = |payload: &[u8]| {
        EchoResponse::new(request_id, responder, payload, 1)
            .expect("bounded reply")
            .into_owned()
    };

    // When
    let exact = echoed(request_id, b"hello", reply(b"hello"));
    let altered = echoed(request_id, b"hello", reply(b"tampered"));
    let not_text = echoed(request_id, b"hello", reply(b"\xff"));

    // Then
    assert!(exact.is_ok_and(|response| response.payload() == b"hello"));
    assert!(matches!(altered, Err(EchoError::InvalidInput)));
    assert!(matches!(not_text, Err(EchoError::InvalidInput)));
}
