use super::{TestResult, no_effect_refusals::Served};
use crate::api;
use ma2a_store::{Repository, StoreConfig};

const TEST_CERT_PEM: &str = "\
-----BEGIN CERTIFICATE-----\n\
MIIBSTCB8KADAgECAhQC5MBKZ9Trm5Lu0sfanwRVCvzQADAKBggqhkjOPQQDAjAU\n\
MRIwEAYDVQQDDAlsb2NhbGhvc3QwIBcNMjAwMTAxMDAwMDAwWhgPMjEyNTAxMDEw\n\
MDAwMDBaMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDBZMBMGByqGSM49AgEGCCqGSM49\n\
AwEHA0IABDREpeqcGCVpLxRtGwn78P2SvMGTmiMRCDopLALFJzeI7FnMYfz/Gyw6\n\
sb31wNUz/SUSDkySCuUYiim3zZultZGjHjAcMBoGA1UdEQQTMBGCCWxvY2FsaG9z\n\
dIcEfwAAATAKBggqhkjOPQQDAgNIADBFAiARKM+U7ZI9SpEiQaqCKcRier6IJYwK\n\
1hWFxg+1MokBWgIhAO+0UOJV0JVoEvPd9Wb/PAk938wBtU+tE7qBuppISAq+\n\
-----END CERTIFICATE-----\n";

const TEST_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgAWjxWXi/raqfjSBG\n\
12uPHOOtx0cJrtRo4yZEDATqi5ahRANCAAQ0RKXqnBglaS8UbRsJ+/D9krzBk5oj\n\
EQg6KSwCxSc3iOxZzGH8/xssOrG99cDVM/0lEg5MkgrlGIopt82bpbWR\n\
-----END PRIVATE KEY-----\n";

pub(super) fn write_tls_files(
    state: &std::path::Path,
) -> TestResult<(std::path::PathBuf, std::path::PathBuf)> {
    let cert_path = state.join("relay.cert.pem");
    let key_path = state.join("relay.key.pem");
    std::fs::write(&cert_path, TEST_CERT_PEM)?;
    std::fs::write(&key_path, TEST_KEY_PEM)?;
    ma2a_store::protect_private_file(&key_path, "test private relay key")?;
    Ok((cert_path, key_path))
}

pub(super) async fn call_json(
    served: &Served,
    request: &serde_json::Value,
) -> TestResult<serde_json::Value> {
    let command = api::decode_command(&serde_json::to_vec(request)?)?;
    Ok(serde_json::from_slice(
        &served.client.call(&command).await?,
    )?)
}

pub(super) fn private_configure_request(
    (request_id, listen, space_id): (&str, &str, &str),
    (cert_path, key_path): (&std::path::Path, &std::path::Path),
) -> TestResult<serde_json::Value> {
    Ok(serde_json::json!({
        "version": 1,
        "operation": "private_relay_configure",
        "request_id": request_id,
        "mode": "native_tls",
        "listen": listen,
        "public_url": "https://localhost/",
        "served_space_ids": [space_id],
        "certificate_path": cert_path.to_str().ok_or("invalid cert path")?,
        "private_key_path": key_path.to_str().ok_or("invalid key path")?,
    }))
}

async fn assert_public_fallback_roundtrip(
    served: &Served,
    config: &StoreConfig,
    (configure_id, disable_id): (&str, &str),
) -> TestResult {
    let configured = call_json(
        served,
        &serde_json::json!({
            "version": 1,
            "operation": "public_relay_configure",
            "request_id": configure_id,
            "url": "https://fallback.example.invalid",
        }),
    )
    .await?;
    assert_eq!(configured.get("error"), None, "{configured}");
    assert_eq!(
        configured.pointer("/result/type"),
        Some(&serde_json::json!("public_relay_configured"))
    );
    let stored = Repository::open(config)?.relay_configuration()?;
    assert!(stored.public_fallback_enabled);
    assert_eq!(
        stored.public_relay_urls,
        vec!["https://fallback.example.invalid/".to_owned()]
    );
    assert_eq!(
        served
            .runtime
            .handle()
            .snapshot()
            .await?
            .to_value()
            .pointer("/public_relay_fallbacks/0/relay_url"),
        Some(&serde_json::json!("https://fallback.example.invalid/"))
    );

    let disabled = call_json(
        served,
        &serde_json::json!({
            "version": 1,
            "operation": "public_relay_disable",
            "request_id": disable_id,
        }),
    )
    .await?;
    assert_eq!(disabled.get("error"), None, "{disabled}");
    assert_eq!(
        disabled.pointer("/result/type"),
        Some(&serde_json::json!("public_relay_status"))
    );
    let stored = Repository::open(config)?.relay_configuration()?;
    assert!(!stored.public_fallback_enabled);
    assert!(stored.public_relay_urls.is_empty());
    assert_eq!(
        served
            .runtime
            .handle()
            .snapshot()
            .await?
            .to_value()
            .pointer("/public_relay_fallbacks"),
        Some(&serde_json::json!([]))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_relay_configure_and_disable_succeed_when_running_private_tls_files_break()
-> TestResult {
    let served = Served::start().await?;
    let config = StoreConfig::new(&served.state.0);
    let space = served
        .runtime
        .handle()
        .create_owned_space("public-running-private".to_owned())
        .await?;
    let (cert_path, key_path) = write_tls_files(&served.state.0)?;
    let space_hex = api::encode_hex(space.as_bytes());
    let private_ok = call_json(
        &served,
        &private_configure_request(
            (&"71".repeat(16), "127.0.0.1:0", &space_hex),
            (&cert_path, &key_path),
        )?,
    )
    .await?;
    assert_eq!(private_ok.get("error"), None, "{private_ok}");
    assert!(
        served
            .runtime
            .handle()
            .relay_status()
            .await?
            .applied_private
            .is_some()
    );

    std::fs::remove_file(&cert_path)?;
    assert_public_fallback_roundtrip(&served, &config, (&"72".repeat(16), &"73".repeat(16)))
        .await?;
    assert!(
        served
            .runtime
            .handle()
            .relay_status()
            .await?
            .applied_private
            .is_some()
    );
    served.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_relay_configure_and_disable_succeed_when_private_relay_is_down_with_broken_tls()
-> TestResult {
    let served = Served::start().await?;
    let config = StoreConfig::new(&served.state.0);
    let space = served
        .runtime
        .handle()
        .create_owned_space("public-down-private".to_owned())
        .await?;
    let (cert_path, key_path) = write_tls_files(&served.state.0)?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let listen = occupied.local_addr()?.to_string();
    let space_hex = api::encode_hex(space.as_bytes());
    let private_failed = call_json(
        &served,
        &private_configure_request(
            (&"74".repeat(16), &listen, &space_hex),
            (&cert_path, &key_path),
        )?,
    )
    .await?;
    assert_eq!(
        private_failed
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("internal")
    );
    let status = served.runtime.handle().relay_status().await?;
    assert!(status.applied_private.is_none());
    assert!(status.convergence_pending);

    std::fs::remove_file(&cert_path)?;
    drop(occupied);
    assert_public_fallback_roundtrip(&served, &config, (&"75".repeat(16), &"76".repeat(16)))
        .await?;
    let after = served.runtime.handle().relay_status().await?;
    assert!(after.applied_private.is_none());
    assert!(after.convergence_pending);
    served.stop().await
}
