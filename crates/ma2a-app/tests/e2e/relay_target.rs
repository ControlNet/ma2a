use std::{sync::Arc, time::Duration};

use iroh::{Endpoint, RelayMode, endpoint::presets};
use iroh_base::{RelayUrl, SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
};
use ma2a_net::{
    AddressLookupClock, AddressRecordTarget, AddressRecordValidator, CONTROL_ALPN,
    SpaceAddressLookup,
};
use ma2a_store::Repository;

use super::harness::{
    NOW_MS, RelayFixture, SpaceFixture, TempState, TestResult, create_space, emit, store_relay,
};

#[derive(Debug)]
struct FixedClock;

impl AddressLookupClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_a_target_resolution_uses_only_the_targets_signed_r2_data() -> TestResult {
    // Given
    let (relay_map, r2, _relay) = iroh::test_utils::run_relay_server().await?;
    let target = SecretKey::from_bytes(&[0x71; 32]);
    let r1_provider = SecretKey::from_bytes(&[0x72; 32]);
    let local = SecretKey::from_bytes(&[0x73; 32]);
    let state = TempState::new("scenario-a")?;
    let mut repository = Repository::open(&state.config())?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &local,
            relays: &[&target, &r1_provider],
            issued_at_ms: 10,
        },
    )?;
    let r1: RelayUrl = "https://r1.example.invalid".parse()?;
    store_relay(
        &mut repository,
        RelayFixture {
            authorization: &authorization,
            relay: &r1_provider,
            relay_url: r1.clone(),
            sequence: 1,
        },
    )?;
    let server = Endpoint::builder(presets::Minimal)
        .secret_key(target.clone())
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .alpns(vec![CONTROL_ALPN.to_vec()])
        .bind()
        .await?;
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(authorization.space_id(), target.public().into()),
        AddressRecordValidity::new(7, NOW_MS - 1, NOW_MS + 60_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Relay(r2.clone())])?,
    )
    .sign(&target)?;
    let validated = AddressRecordValidator::validate_and_store(
        &mut repository,
        signed.canonical_bytes(),
        AddressRecordTarget::new(authorization.space_id(), target.public().into())
            .validation(&authorization, NOW_MS),
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![authorization])?;
    lookup.cache(validated)?;
    let resolved = lookup
        .resolve_endpoint(target.public())
        .ok_or("target lookup returned no data")?;
    let supplied = resolved.data.addrs().cloned().collect::<Vec<_>>();
    let client = Endpoint::builder(presets::Minimal)
        .secret_key(local)
        .relay_mode(RelayMode::Custom(relay_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .clear_address_lookup()
        .address_lookup(lookup)
        .bind()
        .await?;
    let accept = tokio::spawn({
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

    // When
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        client.connect(target.public(), CONTROL_ALPN),
    )
    .await??;

    // Then
    assert_eq!(connection.remote_id(), target.public());
    assert_eq!(supplied, vec![TransportAddr::Relay(r2.clone())]);
    assert!(!supplied.contains(&TransportAddr::Relay(r1.clone())));
    emit(&serde_json::json!({
        "scenario": "A",
        "endpoint_ids": {"target": target.public().to_string()},
        "record_sequences": [7],
        "supplied_relay_candidates": [r2.to_string()],
        "excluded_space_relay": r1.to_string(),
        "iroh_observed_path": "relay",
        "reachability_state": "connected"
    }));
    connection.close(0_u8.into(), b"");
    client.close().await;
    server.close().await;
    let _accepted = accept.await??;
    Ok(())
}
