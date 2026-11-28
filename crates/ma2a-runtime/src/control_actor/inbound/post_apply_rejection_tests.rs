mod fixture;
mod support;

use std::time::Duration;

use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, endpoint::presets};
use ma2a_net::EndpointSecret;
use ma2a_store::{RelayConfiguration, RelayTransportConfiguration};

use crate::control_sync::ControlRoundTrigger;
use fixture::{advance_and_build_push, setup_scenario};
use support::{TestResult, endpoint_hex, send_control_raw, sync_peers};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_revocation_with_cursor_validation_error_reconciles_immediately() -> TestResult {
    let s = setup_scenario("revoke-sender", 0x71).await?;
    s.provider
        .handle()
        .set_relay_configuration(RelayConfiguration {
            public_fallback_enabled: false,
            public_relay_urls: Vec::new(),
            private_provider_enabled: true,
            listener_address: Some("127.0.0.1:0".to_owned()),
            private_relay_url: Some("https://relay.example.invalid/".to_owned()),
            served_spaces: vec![s.space_id],
            transport: Some(RelayTransportConfiguration::ExternalTlsTermination),
        })
        .await?;
    let listen_addr = s
        .provider
        .handle()
        .relay_status()
        .await?
        .private_listen_addr
        .ok_or("private relay not listening")?;
    let relay_url: RelayUrl = format!("http://{listen_addr}").parse()?;
    let relay_probe = Endpoint::builder(presets::Minimal)
        .secret_key(s.peer_a_key.clone())
        .relay_mode(RelayMode::Custom(RelayMap::from(relay_url.clone())))
        .clear_address_lookup()
        .bind()
        .await?;
    tokio::time::timeout(Duration::from_secs(5), relay_probe.online()).await?;

    let (payload, owner_release, owner_blocker) = advance_and_build_push(&s, true, false).await?;

    let (provider_entered, provider_release) = s.provider.handle().arm_control_round_gate();
    let provider_handle = s.provider.handle();
    let dummy_peer = EndpointSecret::generate().endpoint_id();
    let provider_blocker = tokio::spawn(async move {
        let _ = provider_handle.sync_control_with(dummy_peer).await;
    });
    tokio::time::timeout(Duration::from_secs(5), provider_entered).await??;
    let before_snap = s.provider.handle().snapshot().await?.to_value();
    let before_rev = before_snap
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing before revision")?;
    assert!(sync_peers(&before_snap)?.contains(&endpoint_hex(s.peer_a_id)));
    let before_manifest_rounds = s
        .provider
        .handle()
        .control_schedules()
        .iter()
        .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
        .count();

    let (status, body) = send_control_raw(&s.peer_a_key, s.provider_addr.clone(), &payload).await?;
    assert_eq!(status, 2, "expected ControlRejection::Invalid (status 2)");
    assert!(body.is_empty());

    let snap = s.provider.handle().snapshot().await?.to_value();
    let after_rev = snap
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing after revision")?;
    assert!(after_rev > before_rev);
    assert!(
        !sync_peers(&snap)?.contains(&endpoint_hex(s.peer_a_id)),
        "revoked peer remained in synchronized_control_peers after post-apply Invalid"
    );
    let after_manifest_rounds = s
        .provider
        .handle()
        .control_schedules()
        .iter()
        .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
        .count();
    assert!(
        after_manifest_rounds > before_manifest_rounds,
        "ManifestAdvanced round was not scheduled after post-apply Invalid"
    );

    let reconnect_probe = Endpoint::builder(presets::Minimal)
        .secret_key(s.peer_a_key.clone())
        .relay_mode(RelayMode::Custom(RelayMap::from(relay_url)))
        .clear_address_lookup()
        .bind()
        .await?;
    let admitted = tokio::time::timeout(Duration::from_millis(400), reconnect_probe.online()).await;
    assert!(
        admitted.is_err(),
        "revoked peer remained admitted by Private Relay after post-apply Invalid"
    );

    reconnect_probe.close().await;
    relay_probe.close().await;
    let _ = owner_release.send(());
    let _ = provider_release.send(());
    let _ = owner_blocker.await;
    let _ = provider_blocker.await;
    s.peer_a_responder.abort();
    s.peer_a_endpoint.shutdown().await?;
    s.provider.shutdown().await?;
    s.owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_manifest_with_mismatched_cursor_hash_reconciles_immediately() -> TestResult {
    let s = setup_scenario("cursor-mismatch", 0x81).await?;
    let (payload, owner_release, owner_blocker) = advance_and_build_push(&s, false, true).await?;

    let (provider_entered, provider_release) = s.provider.handle().arm_control_round_gate();
    let provider_handle = s.provider.handle();
    let dummy_peer = EndpointSecret::generate().endpoint_id();
    let provider_blocker = tokio::spawn(async move {
        let _ = provider_handle.sync_control_with(dummy_peer).await;
    });
    tokio::time::timeout(Duration::from_secs(5), provider_entered).await??;
    let before_snap = s.provider.handle().snapshot().await?.to_value();
    let before_rev = before_snap
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing before revision")?;
    assert!(sync_peers(&before_snap)?.contains(&endpoint_hex(s.peer_a_id)));
    let before_manifest_rounds = s
        .provider
        .handle()
        .control_schedules()
        .iter()
        .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
        .count();

    let (status, body) = send_control_raw(&s.peer_a_key, s.provider_addr.clone(), &payload).await?;
    assert_eq!(status, 2, "expected ControlRejection::Invalid (status 2)");
    assert!(body.is_empty());

    let snap = s.provider.handle().snapshot().await?.to_value();
    let after_rev = snap
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing after revision")?;
    assert!(after_rev > before_rev);
    assert!(
        !sync_peers(&snap)?.contains(&endpoint_hex(s.peer_a_id)),
        "peer remained in synchronized_control_peers after post-apply cursor mismatch"
    );
    let after_manifest_rounds = s
        .provider
        .handle()
        .control_schedules()
        .iter()
        .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
        .count();
    assert!(
        after_manifest_rounds > before_manifest_rounds,
        "ManifestAdvanced round was not scheduled after post-apply cursor mismatch"
    );

    let _ = owner_release.send(());
    let _ = provider_release.send(());
    let _ = owner_blocker.await;
    let _ = provider_blocker.await;
    s.peer_a_responder.abort();
    s.peer_a_endpoint.shutdown().await?;
    s.provider.shutdown().await?;
    s.owner.shutdown().await?;
    Ok(())
}
