//! Iroh `EndpointData` conversion coverage.

use iroh::address_lookup::{EndpointData, UserData};
use iroh_base::{CustomAddr, TransportAddr};
use ma2a_core::ProtocolError;
use ma2a_net::{address_endpoint_data_from_iroh, address_endpoint_data_to_iroh};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn endpoint_data_conversion_preserves_order_custom_and_user_data() -> TestResult {
    // Given
    let addresses = vec![
        TransportAddr::Custom(CustomAddr::from_parts(13, b"first")),
        TransportAddr::Relay("https://relay.example.test".parse()?),
        TransportAddr::Ip("127.0.0.1:4701".parse()?),
        TransportAddr::Custom(CustomAddr::from_parts(17, b"second")),
    ];
    let iroh = EndpointData::new(addresses.clone())
        .with_user_data(UserData::try_from("metadata".to_owned())?);

    // When
    let bounded = address_endpoint_data_from_iroh(&iroh)?;
    let reconstructed = address_endpoint_data_to_iroh(&bounded)?;

    // Then
    assert_eq!(
        reconstructed.addrs().cloned().collect::<Vec<_>>(),
        addresses
    );
    assert_eq!(
        reconstructed.user_data().map(ToString::to_string),
        Some("metadata".to_owned())
    );
    Ok(())
}

#[test]
fn endpoint_data_conversion_rejects_seventeen_addresses() -> TestResult {
    // Given
    let addresses = (0_u16..17)
        .map(|offset| {
            TransportAddr::Ip(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                4_700_u16 + offset,
            )))
        })
        .collect();
    let iroh = EndpointData::new(addresses);

    // When
    let result = address_endpoint_data_from_iroh(&iroh);

    // Then
    assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    Ok(())
}
