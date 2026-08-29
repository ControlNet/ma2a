//! Live and snapshot address publisher policy coverage.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use iroh::{EndpointAddr, address_lookup::UserData};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_net::{
    AddressPublishRequest, AddressPublisher, AddressPublisherError, EndpointSecret, RuntimeEndpoint,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[test]
fn publisher_rejects_an_observation_owned_by_another_signer() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x54; 32]);
    let other = SecretKey::from_bytes(&[0x55; 32]);
    let mismatch = EndpointAddr::from_parts(
        other.public(),
        [TransportAddr::Ip("127.0.0.1:4103".parse()?)],
    );

    // When
    let result = AddressPublisher::new(signer, mismatch);

    // Then
    assert!(matches!(
        result,
        Err(AddressPublisherError::IdentityMismatch)
    ));
    Ok(())
}

#[test]
fn publisher_advances_sequence_for_a_material_address_change() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x54; 32]);
    let signer_id = signer.public();
    let fixture = space_fixture(&signer, 0x63)?;
    let state = TempState::new("publisher-material-change")?;
    let mut repository = repository(&state, &fixture)?;
    let initial = AddressPublisher::new(
        signer.clone(),
        EndpointAddr::from_parts(signer_id, [TransportAddr::Ip("127.0.0.1:4104".parse()?)]),
    )?;
    initial
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial publication was skipped")?;
    let changed = AddressPublisher::new(
        signer,
        EndpointAddr::from_parts(signer_id, [TransportAddr::Ip("127.0.0.1:4108".parse()?)]),
    )?;

    // When
    let published = changed.publish(
        &mut repository,
        AddressPublishRequest::new(&fixture.authorization, NOW_MS + 1),
    )?;

    // Then
    assert_eq!(
        published
            .ok_or("material address change was skipped")?
            .record()
            .record()
            .sequence(),
        1
    );
    Ok(())
}

#[test]
fn publisher_preserves_custom_addresses_and_user_data() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x58; 32]);
    let fixture = space_fixture(&signer, 0x6a)?;
    let state = TempState::new("publisher-complete-data")?;
    let mut repository = repository(&state, &fixture)?;
    let addresses = [
        TransportAddr::Custom(iroh_base::CustomAddr::from_parts(11, b"opaque")),
        TransportAddr::Ip("127.0.0.1:4111".parse()?),
    ];
    let endpoint_addr = EndpointAddr::from_parts(signer.public(), addresses);
    let expected_addresses = endpoint_addr.addrs.iter().cloned().collect::<Vec<_>>();
    let publisher = AddressPublisher::new_with_user_data(
        signer,
        endpoint_addr,
        Some(UserData::try_from("metadata".to_owned())?),
    )?;

    // When
    let publication = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("complete publication was skipped")?;

    // Then
    assert_eq!(
        publication.record().record().endpoint_data().addresses(),
        expected_addresses
    );
    assert_eq!(
        publication.record().record().endpoint_data().user_data(),
        Some("metadata")
    );
    Ok(())
}

#[test]
fn publisher_refreshes_unchanged_data_after_five_minutes() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x56; 32]);
    let fixture = space_fixture(&signer, 0x68)?;
    let state = TempState::new("publisher-refresh")?;
    let mut repository = repository(&state, &fixture)?;
    let publisher = AddressPublisher::new(
        signer.clone(),
        EndpointAddr::from_parts(
            signer.public(),
            [TransportAddr::Ip("127.0.0.1:4110".parse()?)],
        ),
    )?;
    publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial publication was skipped")?;

    // When
    let refreshed = publisher.publish(
        &mut repository,
        AddressPublishRequest::new(&fixture.authorization, NOW_MS + 300_000),
    )?;

    // Then
    assert_eq!(
        refreshed
            .ok_or("five-minute refresh was skipped")?
            .record()
            .record()
            .sequence(),
        1
    );
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
