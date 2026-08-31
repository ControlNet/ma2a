use std::{sync::Arc, time::Duration};

use iroh::{Endpoint, RelayMode, endpoint::presets};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
};
use ma2a_net::{
    AddressLookupClock, AddressPublishRequest, AddressPublisher, AddressRecordTarget,
    AddressRecordValidator, CONTROL_ALPN, SpaceAddressLookup,
};
use ma2a_store::Repository;

use super::harness::{NOW_MS, SpaceFixture, TempState, TestResult, create_space, emit};

#[derive(Debug)]
struct FixedClock;

impl AddressLookupClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_e_restart_clears_transient_routes_and_fresh_signed_data_connects() -> TestResult {
    // Given
    let (relay_map, relay_url, relay_server) = iroh::test_utils::run_relay_server().await?;
    let server_secret = SecretKey::from_bytes(&[0x78; 32]);
    let client_bytes = [0x79; 32];
    let client_secret = SecretKey::from_bytes(&client_bytes);
    let state = TempState::new("scenario-e")?;
    let mut repository = Repository::open(&state.config())?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &client_secret,
            relays: &[&server_secret],
            issued_at_ms: 70,
        },
    )?;
    let server = Endpoint::builder(presets::Minimal)
        .secret_key(server_secret.clone())
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .alpns(vec![CONTROL_ALPN.to_vec()])
        .bind()
        .await?;
    let initial = Endpoint::builder(presets::Minimal)
        .secret_key(client_secret.clone())
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    let persistent_id: ma2a_core::EndpointId = initial.id().into();
    initial.close().await;
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(authorization.space_id(), server_secret.public().into()),
        AddressRecordValidity::new(1, NOW_MS - 1, NOW_MS + 60_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Relay(relay_url.clone())])?,
    )
    .sign(&server_secret)?;
    let validated = AddressRecordValidator::validate_and_store(
        &mut repository,
        signed.canonical_bytes(),
        AddressRecordTarget::new(authorization.space_id(), server_secret.public().into())
            .validation(&authorization, NOW_MS),
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![authorization])?;
    lookup.cache(validated)?;
    let restarted = Endpoint::builder(presets::Minimal)
        .secret_key(client_secret.clone())
        .relay_mode(RelayMode::Custom(relay_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .clear_address_lookup()
        .address_lookup(lookup)
        .bind()
        .await?;
    let accept = accept_one(server.clone());

    // When
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        restarted.connect(server_secret.public(), CONTROL_ALPN),
    )
    .await??;
    let accepted = accept.await??;

    // Then
    assert_eq!(ma2a_core::EndpointId::from(restarted.id()), persistent_id);
    assert_eq!(connection.remote_id(), server_secret.public());
    assert_eq!(accepted.remote_id(), client_secret.public());
    assert!(selected_relay(&connection, &relay_url));
    emit(&serde_json::json!({
        "scenario": "E",
        "endpoint_ids": {"client": persistent_id.to_public_key()?.to_string(), "target": server_secret.public().to_string()},
        "fresh_signed_target_sequence": 1,
        "selected_relay_path": relay_url.to_string(),
        "connection_remote": connection.remote_id().to_string()
    }));
    connection.close(0_u8.into(), b"");
    restarted.close().await;
    server.close().await;
    drop(relay_server);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_f_observed_home_change_republishes_exact_changed_endpoint_data() -> TestResult {
    // Given
    let (first_map, first_url, first_relay) = iroh::test_utils::run_relay_server().await?;
    let (second_map, second_url, second_relay) = iroh::test_utils::run_relay_server().await?;
    let secret_bytes = [0x7c; 32];
    let secret = SecretKey::from_bytes(&secret_bytes);
    let state = TempState::new("scenario-f")?;
    let mut repository = Repository::open(&state.config())?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &secret,
            relays: &[],
            issued_at_ms: 80,
        },
    )?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret.clone())
        .relay_mode(RelayMode::Custom(first_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    let endpoint_id: ma2a_core::EndpointId = endpoint.id().into();
    let first_observation = super::harness::wait_for_home_status(&endpoint, &first_url).await?;
    let first_endpoint_addr = endpoint.addr();
    let first = AddressPublisher::new(secret.clone(), first_endpoint_addr.clone())?
        .publish(
            &mut repository,
            AddressPublishRequest::new(&authorization, NOW_MS),
        )?
        .ok_or("initial publication missing")?;
    assert!(first_observation);
    let second_config = second_map
        .get(&second_url)
        .ok_or("second relay configuration missing")?;

    // When
    endpoint
        .insert_relay(second_url.clone(), second_config)
        .await;
    endpoint.remove_relay(&first_url).await;
    let second_observation = super::harness::wait_for_home_status(&endpoint, &second_url).await?;
    let second_endpoint_addr = endpoint.addr();
    let second = AddressPublisher::new(secret, second_endpoint_addr.clone())?
        .publish(
            &mut repository,
            AddressPublishRequest::new(&authorization, NOW_MS + 1),
        )?
        .ok_or("changed observation publication missing")?;

    // Then
    assert!(second_observation);
    assert_eq!(ma2a_core::EndpointId::from(endpoint.id()), endpoint_id);
    assert_ne!(first_endpoint_addr.addrs, second_endpoint_addr.addrs);
    assert_eq!(
        second.record().record().endpoint_data().addresses(),
        second_endpoint_addr
            .addrs
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        second.record().record().sequence(),
        first.record().record().sequence() + 1
    );
    emit(&serde_json::json!({
        "scenario": "F",
        "endpoint_ids": {"before": endpoint_id.to_public_key()?.to_string(), "after": endpoint.id().to_string()},
        "record_sequences": [first.record().record().sequence(), second.record().record().sequence()],
        "observed_home_before": first_url.to_string(),
        "observed_home_after": second_url.to_string(),
        "published_endpoint_data_changed": first.record().record().endpoint_data() != second.record().record().endpoint_data()
    }));
    endpoint.close().await;
    drop(first_map);
    drop(first_relay);
    drop(second_relay);
    Ok(())
}

fn accept_one(
    server: Endpoint,
) -> tokio::task::JoinHandle<Result<iroh::endpoint::Connection, std::io::Error>> {
    tokio::spawn(async move {
        let incoming = server
            .accept()
            .await
            .ok_or_else(|| std::io::Error::other("server stopped"))?;
        incoming
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
    })
}

fn selected_relay(connection: &iroh::endpoint::Connection, expected: &iroh_base::RelayUrl) -> bool {
    connection.paths().iter().any(|path| {
        path.is_selected()
            && matches!(path.remote_addr(), TransportAddr::Relay(url) if url == expected)
    })
}
