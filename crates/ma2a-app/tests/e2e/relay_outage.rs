use iroh::{Endpoint, RelayMode, SecretKey, Watcher as _, endpoint::presets};

use super::harness::{TestResult, emit, wait_for_home_status};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_outage_recovers_through_local_replacement_without_public_internet() -> TestResult {
    let secret = SecretKey::from_bytes(&[0x8f; 32]);
    let (first_map, first_url, first_relay) = iroh::test_utils::run_relay_server().await?;
    let (second_map, second_url, second_relay) = iroh::test_utils::run_relay_server().await?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret.clone())
        .relay_mode(RelayMode::Custom(first_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    assert!(wait_for_home_status(&endpoint, &first_url).await?);

    drop(first_relay);
    let second_config = second_map
        .get(&second_url)
        .ok_or("replacement relay config missing")?;
    endpoint
        .insert_relay(second_url.clone(), second_config)
        .await;
    endpoint.remove_relay(&first_url).await;
    assert!(wait_for_home_status(&endpoint, &second_url).await?);
    let homes = endpoint.home_relay_status().get();
    assert!(
        homes
            .iter()
            .any(|status| status.url() == &second_url && status.is_connected())
    );
    emit(&serde_json::json!({
        "scenario": "relay-outage-recovery",
        "endpoint_ids": {"client": secret.public().to_string()},
        "failed_relay": first_url.to_string(),
        "recovered_relay": second_url.to_string(),
        "recovered": true,
        "public_internet_used": false
    }));
    endpoint.close().await;
    drop(second_relay);
    Ok(())
}
