//! Golden byte coverage for the Phase 1 wire contract.

use ma2a_core::{
    EndpointId, ProtocolError, RequestEnvelope, RequestId, ResponseEnvelope, SpaceId,
    decode_request, decode_response, encode_request, encode_response,
};

const ENDPOINT_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];
const REQUEST_BYTES: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

#[test]
fn echo_request_matches_golden_bytes() -> Result<(), ProtocolError> {
    // Given
    let request = RequestEnvelope::echo(request_id()?, endpoint_id()?, b"hello")?;
    let golden = include_bytes!("../testdata/wire-v1/echo-request.cbor");

    // When
    let encoded = encode_request(&request)?;
    let decoded = decode_request(golden)?;
    let reencoded = encode_request(&decoded)?;

    // Then
    assert_eq!(encoded, golden);
    assert_eq!(decoded, request);
    assert_eq!(reencoded, golden);
    Ok(())
}

#[test]
fn echo_response_matches_golden_bytes() -> Result<(), ProtocolError> {
    // Given
    let response = ResponseEnvelope::echo(request_id()?, b"hello")?;
    let golden = include_bytes!("../testdata/wire-v1/echo-response.cbor");

    // When
    let encoded = encode_response(&response)?;
    let decoded = decode_response(golden)?;
    let reencoded = encode_response(&decoded)?;

    // Then
    assert_eq!(encoded, golden);
    assert_eq!(decoded, response);
    assert_eq!(reencoded, golden);
    assert_eq!(decoded.result().echo_payload(), Some(b"hello".as_slice()));
    assert_eq!(decoded.result().error(), None);
    Ok(())
}

#[test]
fn error_response_matches_golden_bytes() -> Result<(), ProtocolError> {
    // Given
    let response = ResponseEnvelope::error(request_id()?, ProtocolError::UNAUTHORIZED);
    let golden = include_bytes!("../testdata/wire-v1/error-response.cbor");

    // When
    let encoded = encode_response(&response)?;
    let decoded = decode_response(golden)?;

    // Then
    assert_eq!(encoded, golden);
    assert_eq!(decoded, response);
    assert_eq!(decoded.result().echo_payload(), None);
    assert_eq!(decoded.result().error(), Some(ProtocolError::UNAUTHORIZED));
    Ok(())
}

#[test]
fn space_id_hashes_domain_then_exact_genesis_bytes() {
    // Given
    let genesis = include_bytes!("../testdata/wire-v1/echo-request.cbor");
    let mut expected_hasher = blake3::Hasher::new();
    expected_hasher.update(b"ma2a-space-v1");
    expected_hasher.update(genesis);

    // When
    let space_id = SpaceId::derive(genesis);

    // Then
    assert_eq!(space_id.as_bytes(), expected_hasher.finalize().as_bytes());
}

#[test]
fn request_id_uses_fresh_random_bytes() -> Result<(), ProtocolError> {
    // Given
    let first = RequestId::random()?;

    // When
    let second = RequestId::random()?;

    // Then
    assert_ne!(first, second);
    Ok(())
}

fn endpoint_id() -> Result<EndpointId, ProtocolError> {
    EndpointId::try_from(ENDPOINT_BYTES.as_slice())
}

fn request_id() -> Result<RequestId, ProtocolError> {
    RequestId::try_from(REQUEST_BYTES.as_slice())
}
