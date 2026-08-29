//! Signed Space address record protocol coverage.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ProtocolError,
    SignedSpaceAddressRecordV1, SpaceAddressRecordV1, SpaceId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn secret(marker: u8) -> SecretKey {
    SecretKey::from_bytes(&[marker; 32])
}

fn record(secret: &SecretKey) -> Result<SpaceAddressRecordV1, ProtocolError> {
    let endpoint_data = AddressEndpointDataV1::new(vec![TransportAddr::Ip(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        4242,
    ))])?;
    let scope = AddressRecordScope::new(
        SpaceId::derive(b"address-record-test-space"),
        secret.public().into(),
    );
    let validity = AddressRecordValidity::new(7, 1_000, 301_000)?;
    Ok(SpaceAddressRecordV1::new(scope, validity, endpoint_data))
}

#[test]
fn canonical_record_roundtrips_and_verifies() -> TestResult {
    // Given
    let signer = secret(0x31);
    let envelope = record(&signer)?.sign(&signer)?;

    // When
    let parsed = SignedSpaceAddressRecordV1::parse_canonical_bytes(envelope.canonical_bytes())?;

    // Then
    parsed.verify_signature()?;
    assert_eq!(parsed, envelope);
    assert_eq!(parsed.record().endpoint_id(), signer.public().into());
    Ok(())
}

#[test]
fn signing_rejects_an_endpoint_identity_mismatch() -> TestResult {
    // Given
    let owner = secret(0x32);
    let other = secret(0x33);

    // When
    let result = record(&owner)?.sign(&other);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn parser_rejects_non_canonical_and_legacy_inner_identity_bytes() -> TestResult {
    // Given
    let signer = secret(0x34);
    let envelope = record(&signer)?.sign(&signer)?;
    let mut trailing = envelope.canonical_bytes().to_vec();
    trailing.push(0);
    let mut legacy_inner_identity = envelope.canonical_bytes().to_vec();
    let body_position = legacy_inner_identity
        .windows(32)
        .position(|window| window == signer.public().as_bytes())
        .ok_or("endpoint identity was not present in the signed body")?;
    legacy_inner_identity.splice(
        body_position..body_position,
        signer.public().as_bytes().iter().copied(),
    );

    // When
    let trailing_result = SignedSpaceAddressRecordV1::parse_canonical_bytes(&trailing);
    let legacy_result = SignedSpaceAddressRecordV1::parse_canonical_bytes(&legacy_inner_identity);

    // Then
    assert_eq!(trailing_result, Err(ProtocolError::INVALID_INPUT));
    assert_eq!(legacy_result, Err(ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn endpoint_data_accepts_empty_and_custom_but_rejects_duplicate_addresses() -> TestResult {
    // Given
    let address = TransportAddr::Ip("127.0.0.1:4242".parse()?);
    let custom = TransportAddr::Custom(iroh_base::CustomAddr::from_parts(1, b"opaque"));

    // When
    let empty = AddressEndpointDataV1::new(Vec::new());
    let duplicate = AddressEndpointDataV1::new(vec![address.clone(), address]);
    let custom = AddressEndpointDataV1::new(vec![custom]);

    // Then
    assert!(empty.is_ok());
    assert_eq!(duplicate, Err(ProtocolError::INVALID_INPUT));
    assert!(custom.is_ok());
    Ok(())
}

#[test]
fn record_rejects_an_overlong_validity_window() {
    // Given

    // When
    let result = AddressRecordValidity::new(1, 1_000, 601_001);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
}
