use std::{sync::Arc, time::Duration};

use iroh::{Endpoint, RelayMode, endpoint::presets};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
};
use ma2a_net::{
    AddressLookupClock, AddressRecordTarget, AddressRecordValidator, CONTROL_ALPN,
    SpaceAddressLookup,
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
    let initial_accept = accept_one(server.clone());
    let learned = tokio::time::timeout(
        Duration::from_secs(10),
        initial.connect(
            iroh::EndpointAddr::from_parts(
                server_secret.public(),
                [TransportAddr::Relay(relay_url.clone())],
            ),
            CONTROL_ALPN,
        ),
    )
    .await??;
    let learned_accepted = initial_accept.await??;
    assert_eq!(learned.remote_id(), server_secret.public());
    assert_eq!(learned_accepted.remote_id(), client_secret.public());
    learned.close(0_u8.into(), b"");
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
        "record_sequences": [1],
        "supplied_relay_candidates": [relay_url.to_string()],
        "iroh_observed_effective_home": relay_url.to_string(),
        "iroh_observed_path": relay_url.to_string(),
        "reachability_state": "RestartedRelayPathConnected",
        "fresh_signed_target_sequence": 1,
        "transient_route_established_before_restart": true,
        "selected_relay_path": relay_url.to_string(),
        "connection_remote": connection.remote_id().to_string()
    }));
    connection.close(0_u8.into(), b"");
    restarted.close().await;
    server.close().await;
    drop(relay_server);
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
