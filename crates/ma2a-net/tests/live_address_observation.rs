//! Verifies live address publication from Iroh endpoint observations.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use iroh::{
    Endpoint, RelayMode,
    address_lookup::{AddressLookup as _, EndpointData, UserData},
    endpoint::presets,
};
use iroh_base::{CustomAddr, SecretKey, TransportAddr};
use ma2a_core::{MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN, ProtocolError};
use ma2a_net::{
    AddressPublishRequest, AddressPublisher, AddressPublisherError, EndpointSecret, RuntimeEndpoint,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[tokio::test]
async fn runtime_publisher_uses_the_running_endpoint_observation() -> TestResult {
    // Given
    let secret_bytes = [0x57; 32];
    let signer = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&signer, 0x69)?;
    let state = TempState::new("live-publisher-observation")?;
    let mut repository = repository(&state, &fixture)?;
    let endpoint = RuntimeEndpoint::bind(EndpointSecret::parse(&secret_bytes)?).await?;
    let observed = endpoint.endpoint_addr();
    let publisher = endpoint.address_publisher()?;

    // When
    let publication = publisher.publish(
        &mut repository,
        AddressPublishRequest::new(&fixture.authorization, NOW_MS),
    )?;

    // Then
    let record = publication.ok_or("live Endpoint publication was skipped")?;
    let observed_addresses = observed.addrs.iter().cloned().collect::<Vec<_>>();
    assert_eq!(
        record.record().record().endpoint_data().addresses(),
        observed_addresses.as_slice()
    );
    endpoint.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn live_publisher_signs_user_data_from_the_iroh_observation() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x59; 32]);
    let fixture = space_fixture(&signer, 0x6b)?;
    let state = TempState::new("live-publisher-user-data")?;
    let mut repository = repository(&state, &fixture)?;
    let endpoint = RuntimeEndpoint::bind(EndpointSecret::parse(&[0x59; 32])?).await?;
    endpoint
        .set_user_data_for_address_lookup(Some(UserData::try_from("live-metadata".to_owned())?));
    let publisher = endpoint.address_publisher()?;

    // When
    let publication = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("live user-data publication was skipped")?;

    // Then
    assert_eq!(
        publication.record().record().endpoint_data().user_data(),
        Some("live-metadata")
    );
    endpoint.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn live_publisher_advances_for_user_data_and_distinguishes_empty_from_absent() -> TestResult {
    // Given
    let secret_bytes = [0x5a; 32];
    let signer = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&signer, 0x6c)?;
    let state = TempState::new("live-publisher-user-data-sequence")?;
    let mut repository = repository(&state, &fixture)?;
    let endpoint = RuntimeEndpoint::bind(EndpointSecret::parse(&secret_bytes)?).await?;
    let publisher = endpoint.address_publisher()?;
    let initial = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial live publication was skipped")?;
    endpoint.set_user_data_for_address_lookup(Some(UserData::try_from(String::new())?));

    // When
    let with_empty = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS + 1),
        )?
        .ok_or("present-empty user data publication was skipped")?;
    endpoint.set_user_data_for_address_lookup(None);
    let absent_again = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS + 2),
        )?
        .ok_or("absent user data publication was skipped")?;

    // Then
    assert_eq!(initial.record().record().sequence(), 0);
    assert_eq!(initial.record().record().endpoint_data().user_data(), None);
    assert_eq!(with_empty.record().record().sequence(), 1);
    assert_eq!(
        with_empty.record().record().endpoint_data().user_data(),
        Some("")
    );
    assert_eq!(absent_again.record().record().sequence(), 2);
    assert_eq!(
        absent_again.record().record().endpoint_data().user_data(),
        None
    );
    endpoint.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn live_publisher_rejects_unavailable_and_oversized_callback_state() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x5b; 32]);
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(signer)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?;
    let lookup = ma2a_net::SpaceAddressLookup::default();

    // When
    let unavailable = AddressPublisher::from_endpoint(&endpoint, &lookup);
    let oversized = vec![0x5b; MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN + 1];
    lookup.publish(&EndpointData::new(vec![TransportAddr::Custom(
        CustomAddr::from_parts(23, &oversized),
    )]));
    let invalid = AddressPublisher::from_endpoint(&endpoint, &lookup);

    // Then
    assert!(matches!(
        unavailable,
        Err(AddressPublisherError::ObservationUnavailable)
    ));
    assert!(matches!(
        invalid,
        Err(AddressPublisherError::ObservationInvalid(error))
            if error == ProtocolError::INVALID_INPUT
    ));
    endpoint.close().await;
    Ok(())
}

#[tokio::test]
async fn live_publisher_preserves_latest_callback_order_and_complete_data() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x5c; 32]);
    let fixture = space_fixture(&signer, 0x6d)?;
    let state = TempState::new("live-publisher-callback-order")?;
    let mut repository = repository(&state, &fixture)?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(signer)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?;
    let lookup = ma2a_net::SpaceAddressLookup::default();
    let addresses = vec![
        TransportAddr::Custom(CustomAddr::from_parts(31, b"first")),
        TransportAddr::Ip("127.0.0.1:4811".parse()?),
        TransportAddr::Custom(CustomAddr::from_parts(32, b"last")),
    ];
    lookup.publish(
        &EndpointData::new(addresses.clone())
            .with_user_data(UserData::try_from("first-metadata".to_owned())?),
    );
    let publisher = AddressPublisher::from_endpoint(&endpoint, &lookup)?;

    // When
    let initial = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial callback publication was skipped")?;
    lookup.publish(
        &EndpointData::new(addresses.clone())
            .with_user_data(UserData::try_from("second-metadata".to_owned())?),
    );
    let updated = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS + 1),
        )?
        .ok_or("updated callback publication was skipped")?;

    // Then
    assert_eq!(
        initial.record().record().endpoint_data().addresses(),
        addresses
    );
    assert_eq!(
        initial.record().record().endpoint_data().user_data(),
        Some("first-metadata")
    );
    assert_eq!(updated.record().record().sequence(), 1);
    assert_eq!(
        updated.record().record().endpoint_data().addresses(),
        addresses
    );
    assert_eq!(
        updated.record().record().endpoint_data().user_data(),
        Some("second-metadata")
    );
    endpoint.close().await;
    Ok(())
}

fn repository(
    state: &TempState,
    fixture: &support::SpaceFixture,
) -> Result<Repository, Box<dyn std::error::Error + Send + Sync>> {
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    Ok(repository)
}
