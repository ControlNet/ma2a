//! Adversarial and property coverage for closed ontology and wire parsing.

use ma2a_core::{
    EndpointId, MAX_PAYLOAD_LEN, ProtocolError, RequestEnvelope, RequestId, RuntimeIdentity,
    SpaceId, SpaceMembership, decode_request, encode_request,
};
use proptest::prelude::*;

const GOLDEN: &[u8] = include_bytes!("../testdata/wire-v1/echo-request.cbor");

proptest! {
    #[test]
    fn rejects_unknown_major_versions(major in 2u8..=23) {
        // Given
        let mut bytes = GOLDEN.to_vec();
        prop_assert!(replace_byte(&mut bytes, 3, major));

        // When
        let result = decode_request(&bytes);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::VERSION_MISMATCH));
    }

    #[test]
    fn rejects_out_of_order_or_unknown_first_fields(key in 1u8..=23) {
        // Given
        let mut bytes = GOLDEN.to_vec();
        prop_assert!(replace_byte(&mut bytes, 1, key));

        // When
        let result = decode_request(&bytes);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn rejects_duplicate_fields(duplicate_key in 0u8..=0) {
        // Given
        let mut bytes = GOLDEN.to_vec();
        prop_assert!(replace_byte(&mut bytes, 5, duplicate_key));

        // When
        let result = decode_request(&bytes);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn rejects_wrong_request_id_lengths(length in 0u8..=23) {
        prop_assume!(length != 16);
        // Given
        let mut bytes = GOLDEN.to_vec();
        prop_assert!(replace_byte(&mut bytes, 6, 0x40 | length));

        // When
        let result = decode_request(&bytes);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn rejects_wrong_endpoint_id_lengths(length in 0u8..=31) {
        // Given
        let mut bytes = GOLDEN.to_vec();
        prop_assert!(replace_byte(&mut bytes, 25, length));

        // When
        let result = decode_request(&bytes);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn endpoint_identity_never_accepts_embedded_space_membership(bytes in prop::collection::vec(any::<u8>(), 64..=96)) {
        // Given
        let identity_with_membership = bytes.as_slice();

        // When
        let result = EndpointId::try_from(identity_with_membership);

        // Then
        prop_assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn arbitrary_wire_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4_161)) {
        // Given / When
        let result = decode_request(&bytes);

        // Then
        prop_assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn rejects_noncanonical_integer_encoding() {
    // Given
    let mut bytes = GOLDEN.to_vec();
    bytes.splice(1..2, [0x18, 0x00]);

    // When
    let result = decode_request(&bytes);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
}

#[test]
fn rejects_unknown_fields() {
    // Given
    let mut bytes = GOLDEN.to_vec();
    assert!(replace_byte(&mut bytes, 0, 0xa5));

    // When
    let result = decode_request(&bytes);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
}

#[test]
fn rejects_indefinite_payloads() {
    // Given
    let mut bytes = GOLDEN.to_vec();
    assert!(replace_byte(&mut bytes, 61, 0x5f));

    // When
    let result = decode_request(&bytes);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
}

#[test]
fn rejects_trailing_data() {
    // Given
    let mut bytes = GOLDEN.to_vec();
    bytes.push(0);

    // When
    let result = decode_request(&bytes);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
}

#[test]
fn rejects_oversize_wire_payload_before_reading_payload() -> Result<(), ProtocolError> {
    // Given
    let prefix = GOLDEN.get(..61).ok_or(ProtocolError::INVALID_INPUT)?;
    let mut bytes = Vec::with_capacity(MAX_PAYLOAD_LEN + 65);
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(&[0x59, 0x10, 0x01]);
    bytes.resize(MAX_PAYLOAD_LEN + 64, 0);

    // When
    let result = decode_request(&bytes);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn rejects_oversize_payload_before_envelope_construction() -> Result<(), ProtocolError> {
    // Given
    let payload = vec![0; MAX_PAYLOAD_LEN + 1];
    let endpoint = EndpointId::try_from(golden_endpoint_bytes()?)?;
    let request_id = RequestId::try_from([0u8; 16].as_slice())?;

    // When
    let result = RequestEnvelope::echo(request_id, endpoint, &payload);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn runtime_identity_and_space_membership_remain_separate() -> Result<(), ProtocolError> {
    // Given
    let endpoint = EndpointId::try_from(golden_endpoint_bytes()?)?;
    let runtime = RuntimeIdentity::new(endpoint);
    let spaces = [SpaceId::derive(b"alpha"), SpaceId::derive(b"beta")];

    // When
    let memberships = spaces.map(|space_id| SpaceMembership::new(space_id, endpoint));

    // Then
    assert_eq!(runtime.endpoint_id(), endpoint);
    assert!(
        memberships
            .iter()
            .all(|value| value.endpoint_id() == endpoint)
    );
    assert_eq!(memberships.map(SpaceMembership::space_id), spaces,);
    Ok(())
}

#[test]
fn maximum_payload_round_trips_without_extra_space_context() -> Result<(), ProtocolError> {
    // Given
    let payload = vec![0x5a; MAX_PAYLOAD_LEN];
    let request = RequestEnvelope::echo(
        RequestId::try_from([0u8; 16].as_slice())?,
        EndpointId::try_from(golden_endpoint_bytes()?)?,
        &payload,
    )?;

    // When
    let encoded = encode_request(&request)?;
    let decoded = decode_request(&encoded)?;

    // Then
    assert_eq!(decoded, request);
    Ok(())
}

fn replace_byte(bytes: &mut [u8], index: usize, value: u8) -> bool {
    bytes.get_mut(index).is_some_and(|slot| {
        *slot = value;
        true
    })
}

fn golden_endpoint_bytes() -> Result<&'static [u8], ProtocolError> {
    GOLDEN.get(26..58).ok_or(ProtocolError::INVALID_INPUT)
}
