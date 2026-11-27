use std::sync::atomic::Ordering;

use tokio::time::timeout;

use super::{
    NOW_MS, PROMPT, TestResult,
    mutations_test::{expect_ipc_ok, spawn_ipc, start_clock},
};
use crate::{api, ipc::LocalApiClient};

async fn assert_private_relay_online(client: &LocalApiClient) -> TestResult {
    let status_cmd = api::decode_command(br#"{"version":1,"operation":"private_relay_status"}"#)?;
    let status_json: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&status_cmd)).await??)?;
    assert_eq!(
        status_json.pointer("/result/payload/online"),
        Some(&serde_json::json!(true)),
        "private relay should stay online under ClockBehind: {status_json}"
    );
    assert!(
        status_json
            .pointer("/result/payload/port")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|port| port > 0),
        "private relay should report its live port under ClockBehind: {status_json}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_configuration_mutations_succeed_over_ipc_under_clock_behind() -> TestResult {
    let (dir, clock, runtime) = start_clock("a04-relay").await?;
    let handle = runtime.handle();

    clock.0.store(NOW_MS + 30_000, Ordering::Relaxed);
    let blocked = handle.create_owned_space("blocked".to_owned()).await?;
    handle
        .set_relay_configuration(ma2a_store::RelayConfiguration {
            public_fallback_enabled: false,
            public_relay_urls: Vec::new(),
            private_provider_enabled: true,
            listener_address: Some("127.0.0.1:0".to_owned()),
            private_relay_url: Some("https://initial.example.invalid/".to_owned()),
            served_spaces: vec![blocked],
            transport: Some(ma2a_store::RelayTransportConfiguration::ExternalTlsTermination),
        })
        .await?;

    clock.0.store(NOW_MS, Ordering::Relaxed);
    let fresh = handle.create_owned_space("fresh".to_owned()).await?;
    let (client, stop, server_task) = spawn_ipc(&dir.0, handle.clone()).await?;

    expect_ipc_ok(
        (&client, &handle),
        serde_json::json!({
            "version": 1,
            "operation": "public_relay_configure",
            "request_id": "c1".repeat(16),
            "url": "https://public.example.invalid",
        }),
        Some("public_relay_configured"),
    )
    .await?;
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/public_relay_fallbacks/0/relay_url"),
        Some(&serde_json::json!("https://public.example.invalid/"))
    );
    assert_private_relay_online(&client).await?;

    expect_ipc_ok(
        (&client, &handle),
        serde_json::json!({
            "version": 1,
            "operation": "private_relay_configure",
            "request_id": "c2".repeat(16),
            "mode": "external_termination",
            "listen": "127.0.0.1:0",
            "public_url": "https://private.example.invalid",
            "served_space_ids": [
                api::encode_hex(blocked.as_bytes()),
                api::encode_hex(fresh.as_bytes()),
            ],
            "certificate_path": null,
            "private_key_path": null,
        }),
        Some("private_relay_configured"),
    )
    .await?;
    assert_private_relay_online(&client).await?;

    expect_ipc_ok(
        (&client, &handle),
        serde_json::json!({
            "version": 1,
            "operation": "public_relay_disable",
            "request_id": "c3".repeat(16),
        }),
        Some("public_relay_status"),
    )
    .await?;
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/public_relay_fallbacks"),
        Some(&serde_json::json!([]))
    );

    stop.cancel();
    let _ = server_task.await;
    timeout(PROMPT, runtime.shutdown()).await??;
    Ok(())
}
