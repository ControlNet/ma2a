//! Full bounded identity-free endpoint data coverage.

use iroh_base::SecretKey;
use iroh_base::{CustomAddr, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity,
    MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN, MAX_ADDRESS_RECORD_USER_DATA_LEN, ProtocolError,
    SignedSpaceAddressRecordV1, SpaceAddressRecordV1, SpaceId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn endpoint_data_accepts_custom_addresses_without_reordering() -> TestResult {
    // Given
    let addresses = vec![
        TransportAddr::Ip("[::1]:4401".parse()?),
        TransportAddr::Custom(CustomAddr::from_parts(0x0054_4f52, b"opaque")),
        TransportAddr::Relay("https://relay.example.test".parse()?),
        TransportAddr::Ip("127.0.0.1:4402".parse()?),
    ];

    // When
    let endpoint_data = AddressEndpointDataV1::new(addresses.clone())?;

    // Then
    assert_eq!(endpoint_data.addresses(), addresses);
    Ok(())
}

#[test]
fn signed_endpoint_data_roundtrips_all_supported_variants() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x91; 32]);
    let addresses = vec![
        TransportAddr::Relay("https://relay-a.example.test".parse()?),
        TransportAddr::Ip("127.0.0.1:4403".parse()?),
        TransportAddr::Ip("[::1]:4404".parse()?),
        TransportAddr::Custom(CustomAddr::from_parts(7, b"first")),
        TransportAddr::Custom(CustomAddr::from_parts(9, b"second")),
    ];
    let endpoint_data =
        AddressEndpointDataV1::from_parts(addresses.clone(), Some("metadata".to_owned()))?;
    let record = SpaceAddressRecordV1::new(
        AddressRecordScope::new(
            SpaceId::derive(b"full-endpoint-data"),
            signer.public().into(),
        ),
        AddressRecordValidity::new(3, 1_000, 601_000)?,
        endpoint_data,
    )
    .sign(&signer)?;

    // When
    let parsed = SignedSpaceAddressRecordV1::parse_canonical_bytes(record.canonical_bytes())?;

    // Then
    assert_eq!(parsed.record().endpoint_data().addresses(), addresses);
    assert_eq!(
        parsed.record().endpoint_data().user_data(),
        Some("metadata")
    );
    assert_eq!(parsed.canonical_bytes(), record.canonical_bytes());
    Ok(())
}

#[test]
fn endpoint_data_distinguishes_absent_and_present_empty_user_data() -> TestResult {
    // Given
    let absent = AddressEndpointDataV1::from_parts(Vec::new(), None)?;
    let present = AddressEndpointDataV1::from_parts(Vec::new(), Some(String::new()))?;

    // When
    let absent_user_data = absent.user_data();
    let present_user_data = present.user_data();

    // Then
    assert_eq!(absent_user_data, None);
    assert_eq!(present_user_data, Some(""));
    assert_ne!(absent, present);
    Ok(())
}

#[test]
fn endpoint_data_enforces_custom_and_user_data_byte_bounds() {
    // Given
    let custom_max = vec![0x5a; MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN];
    let custom_too_large = vec![0x5a; MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN + 1];
    let user_max = "a".repeat(MAX_ADDRESS_RECORD_USER_DATA_LEN);
    let user_too_large = "a".repeat(MAX_ADDRESS_RECORD_USER_DATA_LEN + 1);
    let multibyte_too_large = "é".repeat(123);

    // When
    let empty_custom =
        AddressEndpointDataV1::new(vec![TransportAddr::Custom(CustomAddr::from_parts(1, &[]))]);
    let maximum_custom = AddressEndpointDataV1::new(vec![TransportAddr::Custom(
        CustomAddr::from_parts(2, &custom_max),
    )]);
    let oversized_custom = AddressEndpointDataV1::new(vec![TransportAddr::Custom(
        CustomAddr::from_parts(3, &custom_too_large),
    )]);
    let maximum_user = AddressEndpointDataV1::from_parts(Vec::new(), Some(user_max));
    let oversized_user = AddressEndpointDataV1::from_parts(Vec::new(), Some(user_too_large));
    let oversized_multibyte =
        AddressEndpointDataV1::from_parts(Vec::new(), Some(multibyte_too_large));

    // Then
    assert!(empty_custom.is_ok());
    assert!(maximum_custom.is_ok());
    assert_eq!(oversized_custom, Err(ProtocolError::INVALID_INPUT));
    assert!(maximum_user.is_ok());
    assert_eq!(oversized_user, Err(ProtocolError::INVALID_INPUT));
    assert_eq!(oversized_multibyte, Err(ProtocolError::INVALID_INPUT));
}
