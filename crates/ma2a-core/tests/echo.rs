//! Echo v1 core contract tests.

use ma2a_core::{
    EchoError, EchoRequest, EchoResponse, EchoStatus, EndpointId, MAX_ECHO_PAYLOAD_LEN, RequestId,
};

const ENDPOINT_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

#[test]
fn request_round_trips_exact_payload_when_within_bound() -> Result<(), Box<dyn std::error::Error>> {
    // Given
    let target = EndpointId::try_from(ENDPOINT_BYTES.as_slice())?;
    let request_id = RequestId::try_from([0x17; 16].as_slice())?;
    let payload = [0x00, 0xff, 0x7f, 0x80];

    // When
    let encoded = EchoRequest::new(request_id, target, &payload)?.encode()?;
    let decoded = EchoRequest::decode(&encoded)?;

    // Then
    assert_eq!(decoded.request_id(), request_id);
    assert_eq!(decoded.target_endpoint_id(), target);
    assert_eq!(decoded.payload(), payload);
    Ok(())
}

#[test]
fn request_rejects_4097_byte_payload() -> Result<(), Box<dyn std::error::Error>> {
    // Given
    let target = EndpointId::try_from(ENDPOINT_BYTES.as_slice())?;
    let request_id = RequestId::try_from([0x18; 16].as_slice())?;
    let payload = vec![0x5a; MAX_ECHO_PAYLOAD_LEN + 1];

    // When
    let result = EchoRequest::new(request_id, target, &payload);

    // Then
    assert_eq!(result.err(), Some(EchoError::InvalidInput));
    Ok(())
}

#[test]
fn response_carries_responder_timing_status_and_exact_payload()
-> Result<(), Box<dyn std::error::Error>> {
    // Given
    let responder = EndpointId::try_from(ENDPOINT_BYTES.as_slice())?;
    let request_id = RequestId::try_from([0x19; 16].as_slice())?;
    let payload = [0x61, 0x00, 0x62];

    // When
    let response = EchoResponse::new(request_id, responder, &payload, 37)?;

    // Then
    assert_eq!(response.request_id(), request_id);
    assert_eq!(response.responder_endpoint_id(), responder);
    assert_eq!(response.payload(), payload);
    assert_eq!(response.duration_ms(), 37);
    assert_eq!(response.status(), EchoStatus::Ok);
    Ok(())
}
