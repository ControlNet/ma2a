//! End-to-end exact-target dialing and path telemetry coverage.

#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use std::time::Duration;

use iroh::{Endpoint, RelayMode, endpoint::presets};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidator, CONTROL_ALPN, ConnectionPathState, DialRequest,
    EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_endpoint_dials_exact_target_record_and_observes_direct_path() -> TestResult {
    // Given
    let server_secret = SecretKey::from_bytes(&[0x51; 32]);
    let fixture = space_fixture(&server_secret, 0x52)?;
    let state = TempState::new("connection-cold-start")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let (server_enrollment, _server_enrollment_rx) = tokio::sync::mpsc::channel(1);
    let (server_control, _server_control_rx) = tokio::sync::mpsc::channel(1);
    let server = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&[0x51; 32])?,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(server_enrollment, None).with_control(server_control, true),
    )
    .await?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), server.endpoint_id());
    let record = SpaceAddressRecordV1::new(
        AddressRecordScope::new(fixture.genesis.space_id(), server.endpoint_id()),
        AddressRecordValidity::new(1, NOW_MS - 1, NOW_MS + 60_000)?,
        server.endpoint_data()?,
    )
    .sign(&server_secret)?;
    let validated = AddressRecordValidator::validate_and_store(
        &mut repository,
        record.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    )?;
    let lookup = SpaceAddressLookup::with_clock(std::sync::Arc::new(FixedClock));
    lookup.replace_authorizations(vec![fixture.authorization.clone()])?;
    lookup.cache(validated.clone())?;
    let (client_enrollment, _client_enrollment_rx) = tokio::sync::mpsc::channel(1);
    let client = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&[0x53; 32])?,
        lookup,
        EndpointBindOptions::new(client_enrollment, None),
    )
    .await?;
    let manager = client.connection_manager();

    // When
    let connection = manager
        .connect(DialRequest::new(
            server.endpoint_id(),
            CONTROL_ALPN.to_vec(),
        ))
        .await?;
    let observations = manager.telemetry().observations(server.endpoint_id());

    // Then
    assert_eq!(connection.remote_id(), server_secret.public());
    assert_eq!(connection.alpn(), CONTROL_ALPN);
    assert!(
        observations
            .iter()
            .any(|item| item.path_state() == ConnectionPathState::Connecting)
    );
    assert!(
        observations
            .iter()
            .any(|item| item.path_state() == ConnectionPathState::Direct)
    );
    assert!(
        observations
            .iter()
            .all(|item| item.remote_endpoint_id() == server.endpoint_id())
    );
    let bind_port = server.bind_port()?;
    connection.close(0_u8.into(), b"");
    client.shutdown().await?;
    server.shutdown().await?;

    let (restarted_server_enrollment, _restarted_server_enrollment_rx) =
        tokio::sync::mpsc::channel(1);
    let (restarted_server_control, _restarted_server_control_rx) = tokio::sync::mpsc::channel(1);
    let restarted_server = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&[0x51; 32])?,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(restarted_server_enrollment, Some(bind_port))
            .with_control(restarted_server_control, true),
    )
    .await?;
    let restarted_lookup = SpaceAddressLookup::with_clock(std::sync::Arc::new(FixedClock));
    restarted_lookup.replace_authorizations(vec![fixture.authorization])?;
    restarted_lookup.cache(validated)?;
    let (restarted_client_enrollment, _restarted_client_enrollment_rx) =
        tokio::sync::mpsc::channel(1);
    let restarted_client = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&[0x54; 32])?,
        restarted_lookup,
        EndpointBindOptions::new(restarted_client_enrollment, None),
    )
    .await?;

    let restarted_connection = restarted_client
        .connection_manager()
        .connect(DialRequest::new(
            restarted_server.endpoint_id(),
            CONTROL_ALPN.to_vec(),
        ))
        .await?;

    assert_eq!(restarted_connection.remote_id(), server_secret.public());
    restarted_connection.close(0_u8.into(), b"");
    restarted_client.shutdown().await?;
    restarted_server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_iroh_relay_path_upgrades_to_direct_without_ma2a_routing() -> TestResult {
    let (relay_map, relay_url, _relay_server) = iroh::test_utils::run_relay_server().await?;
    let server_secret = SecretKey::from_bytes(&[0x55; 32]);
    let server = Endpoint::builder(presets::Minimal)
        .secret_key(server_secret.clone())
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .alpns(vec![CONTROL_ALPN.to_vec()])
        .bind()
        .await?;
    let lookup = SpaceAddressLookup::with_clock(std::sync::Arc::new(FixedClock));
    let fixture = space_fixture(&server_secret, 0x56)?;
    let state = TempState::new("connection-relay-upgrade")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let target =
        AddressRecordTarget::new(fixture.genesis.space_id(), server_secret.public().into());
    let record = SpaceAddressRecordV1::new(
        AddressRecordScope::new(fixture.genesis.space_id(), server_secret.public().into()),
        AddressRecordValidity::new(1, NOW_MS - 1, NOW_MS + 60_000)?,
        ma2a_core::AddressEndpointDataV1::from_parts(
            vec![TransportAddr::Relay(relay_url.clone())],
            None,
        )?,
    )
    .sign(&server_secret)?;
    let validated = AddressRecordValidator::validate_and_store(
        &mut repository,
        record.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    )?;
    lookup.replace_authorizations(vec![fixture.authorization])?;
    lookup.cache(validated)?;
    let client = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[0x57; 32]))
        .relay_mode(RelayMode::Custom(relay_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .clear_address_lookup()
        .address_lookup(lookup)
        .bind()
        .await?;
    let accept_task = tokio::spawn({
        let server = server.clone();
        async move {
            let incoming = server
                .accept()
                .await
                .ok_or_else(|| std::io::Error::other("server stopped"))?;
            incoming
                .await
                .map_err(|error| std::io::Error::other(error.to_string()))
        }
    });
    let manager = ma2a_net::ConnectionManager::new(client.clone());

    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        manager.connect(DialRequest::new(
            server_secret.public().into(),
            CONTROL_ALPN.to_vec(),
        )),
    )
    .await??;
    let initial_has_relay = connection
        .paths()
        .iter()
        .any(|path| matches!(path.remote_addr(), TransportAddr::Relay(url) if url == &relay_url));
    let upgraded = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if connection.paths().iter().any(|path| {
                path.is_selected() && matches!(path.remote_addr(), TransportAddr::Ip(_))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;

    assert!(initial_has_relay);
    assert!(upgraded.is_ok());
    connection.close(0_u8.into(), b"");
    manager.shutdown().await;
    client.close().await;
    server.close().await;
    let _accepted = accept_task.await??;
    Ok(())
}

#[derive(Debug)]
struct FixedClock;

impl ma2a_net::AddressLookupClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}
