//! Verifies live address publication from Iroh endpoint observations.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use std::time::Duration;

use iroh::address_lookup::{AddressLookup as _, EndpointData, UserData};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_net::{
    AddressPublishRequest, EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[tokio::test]
async fn runtime_publisher_ignores_callbacks_injected_through_the_public_lookup() -> TestResult {
    // Given
    let secret_bytes = [0x5d; 32];
    let signer = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&signer, 0x6e)?;
    let state = TempState::new("runtime-publisher-provenance")?;
    let mut repository = repository(&state, &fixture)?;
    let lookup = SpaceAddressLookup::default();
    let retained_lookup = lookup.clone();
    let (enrollment_sender, _enrollment_receiver) = tokio::sync::mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&secret_bytes)?,
        lookup,
        EndpointBindOptions::new(enrollment_sender, None),
    )
    .await?;
    let injected = TransportAddr::Ip("127.0.0.1:4812".parse()?);
    retained_lookup.publish(&EndpointData::new(vec![injected.clone()]));
    let publisher = endpoint.address_publisher()?;

    // When
    let publication = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("runtime provenance publication was skipped")?;

    // Then
    assert!(
        !publication
            .record()
            .record()
            .endpoint_data()
            .addresses()
            .contains(&injected)
    );
    endpoint.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_publisher_uses_the_running_endpoint_observation() -> TestResult {
    // Given
    let secret_bytes = [0x57; 32];
    let signer = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&signer, 0x69)?;
    let state = TempState::new("live-publisher-observation")?;
    let mut repository = repository(&state, &fixture)?;
    let (enrollment_sender, _enrollment_receiver) = tokio::sync::mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind(
        EndpointSecret::parse(&secret_bytes)?,
        enrollment_sender,
        None,
    )
    .await?;
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
    let (enrollment_sender, _enrollment_receiver) = tokio::sync::mpsc::channel(1);
    let endpoint =
        RuntimeEndpoint::bind(EndpointSecret::parse(&[0x59; 32])?, enrollment_sender, None).await?;
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
    let (enrollment_sender, _enrollment_receiver) = tokio::sync::mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind(
        EndpointSecret::parse(&secret_bytes)?,
        enrollment_sender,
        None,
    )
    .await?;
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
async fn relay_observer_publishes_exact_next_record_for_user_data_only_change() -> TestResult {
    // Given
    let secret_bytes = [0x5b; 32];
    let signer = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&signer, 0x6d)?;
    let state = TempState::new("relay-observer-user-data")?;
    let mut repository = repository(&state, &fixture)?;
    let (enrollment_sender, _enrollment_receiver) = tokio::sync::mpsc::channel(1);
    let endpoint = RuntimeEndpoint::bind(
        EndpointSecret::parse(&secret_bytes)?,
        enrollment_sender,
        None,
    )
    .await?;
    let publisher = endpoint.address_publisher()?;
    let (observation_sender, mut observations) = tokio::sync::mpsc::channel(2);
    let observer = endpoint.spawn_relay_observer(observation_sender);
    let initial_observation = observations
        .recv()
        .await
        .ok_or("initial relay observation channel closed")?;
    let initial = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial relay-observer publication was skipped")?;

    // When
    endpoint.set_user_data_for_address_lookup(Some(UserData::try_from(
        "effective-metadata".to_owned(),
    )?));
    let updated_observation = tokio::time::timeout(Duration::from_secs(2), observations.recv())
        .await?
        .ok_or("updated relay observation channel closed")?;
    let updated = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS + 1),
        )?
        .ok_or("updated relay-observer publication was skipped")?;

    // Then
    assert_eq!(initial_observation.endpoint_addr().id, signer.public());
    assert_eq!(updated_observation.endpoint_addr().id, signer.public());
    assert_eq!(
        updated.record().record().sequence(),
        initial.record().record().sequence() + 1
    );
    assert_eq!(
        updated.record().record().endpoint_id(),
        initial.record().record().endpoint_id()
    );
    assert_eq!(
        updated.record().record().endpoint_data().user_data(),
        Some("effective-metadata")
    );
    endpoint.shutdown().await?;
    observer.await?;
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
