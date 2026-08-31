use std::{net::Ipv4Addr, time::Duration};

use iroh::{Endpoint, EndpointAddr, RelayMode, endpoint::presets};
use iroh_base::{SecretKey, TransportAddr};
use ma2a_net::CONTROL_ALPN;
use ma2a_net::{
    PrivateRelayAccess, PrivateRelayProviderConfig, PrivateRelayProviderLocation,
    PrivateRelayServer, PrivateRelayTransport,
};

use super::harness::{
    SpaceFixture, TempState, TestResult, create_space, emit, observed_home_endpoint,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_c_members_of_either_space_connect_through_the_common_private_relay() -> TestResult
{
    // Given
    let provider = SecretKey::from_bytes(&[0x77; 32]);
    let peer_x = SecretKey::from_bytes(&[0x7a; 32]);
    let peer_y = SecretKey::from_bytes(&[0x7b; 32]);
    let state_x = TempState::new("scenario-c-x")?;
    let state_y = TempState::new("scenario-c-y")?;
    let authorization_x = create_space(
        &mut ma2a_store::Repository::open(&state_x.config())?,
        SpaceFixture {
            local: &peer_x,
            relays: &[&provider],
            issued_at_ms: 50,
        },
    )?;
    let authorization_y = create_space(
        &mut ma2a_store::Repository::open(&state_y.config())?,
        SpaceFixture {
            local: &peer_y,
            relays: &[&provider],
            issued_at_ms: 60,
        },
    )?;
    let served_spaces = [authorization_x.space_id(), authorization_y.space_id()];
    let access = PrivateRelayAccess::new(provider.public().into(), &served_spaces);
    access.replace_from_spaces(&[authorization_x.clone(), authorization_y.clone()]);
    assert!(access.admits(peer_x.public().into()));
    assert!(access.admits(peer_y.public().into()));
    let relay_config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse()?,
        ),
        served_spaces.to_vec(),
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let relay_server = PrivateRelayServer::spawn(&relay_config, access).await?;
    let relay_url: iroh_base::RelayUrl =
        format!("http://{}", relay_server.listen_addr()).parse()?;
    let relay_map: iroh_relay::RelayMap = std::iter::once(relay_url.clone()).collect();
    let server = Endpoint::builder(presets::Minimal)
        .secret_key(provider.clone())
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .alpns(vec![CONTROL_ALPN.to_vec()])
        .bind()
        .await?;
    let alice_client = observed_home_endpoint(&peer_x, relay_map.clone(), &relay_url).await?;
    let bob_client = observed_home_endpoint(&peer_y, relay_map, &relay_url).await?;
    let target =
        EndpointAddr::from_parts(provider.public(), [TransportAddr::Relay(relay_url.clone())]);

    // When
    let accept_x = accept_one(server.clone());
    let connection_x = tokio::time::timeout(
        Duration::from_secs(10),
        alice_client.connect(target.clone(), CONTROL_ALPN),
    )
    .await??;
    let accepted_x = accept_x.await??;
    let accept_y = accept_one(server.clone());
    let connection_y = tokio::time::timeout(
        Duration::from_secs(10),
        bob_client.connect(target, CONTROL_ALPN),
    )
    .await??;
    let accepted_y = accept_y.await??;

    // Then
    assert_eq!(connection_x.remote_id(), provider.public());
    assert_eq!(connection_y.remote_id(), provider.public());
    assert_eq!(accepted_x.remote_id(), peer_x.public());
    assert_eq!(accepted_y.remote_id(), peer_y.public());
    assert!(authorization_x.contains_member(peer_x.public().into()));
    assert!(authorization_y.contains_member(peer_y.public().into()));
    assert!(has_relay_path(&connection_x, &relay_url));
    assert!(has_relay_path(&connection_y, &relay_url));
    emit(&serde_json::json!({
        "scenario": "C",
        "endpoint_ids": {
            "provider": provider.public().to_string(),
            "space_x_peer": peer_x.public().to_string(),
            "space_y_peer": peer_y.public().to_string()
        },
        "space_ids": [format!("{:?}", authorization_x.space_id()), format!("{:?}", authorization_y.space_id())],
        "record_sequences": [],
        "supplied_relay_candidates": [relay_url.to_string()],
        "iroh_observed_effective_home": relay_url.to_string(),
        "iroh_observed_path": relay_url.to_string(),
        "reachability_state": "CommonPrivateRelayConnected",
        "public_fallback_enabled": false,
        "selected_relay_paths": [relay_url.to_string(), relay_url.to_string()]
    }));
    connection_x.close(0_u8.into(), b"");
    connection_y.close(0_u8.into(), b"");
    alice_client.close().await;
    bob_client.close().await;
    server.close().await;
    relay_server.shutdown().await?;
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

fn has_relay_path(connection: &iroh::endpoint::Connection, expected: &iroh_base::RelayUrl) -> bool {
    connection
        .paths()
        .iter()
        .any(|path| matches!(path.remote_addr(), TransportAddr::Relay(url) if url == expected))
}
