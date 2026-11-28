use super::{
    super::relay_public_fallback::{call_json, private_configure_request, write_tls_files},
    Served, TestResult,
};
use crate::api;
use ma2a_core::ProtocolError;
use ma2a_store::{MutationReplayState, StoreConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_commit_native_tls_load_failure_releases_request_id_for_retry() -> TestResult {
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("tls-precommit-retry".to_owned())
        .await?;
    let cert_path = served.state.0.join("relay.cert.pem");
    let key_path = served.state.0.join("relay.key.pem");
    let request = private_configure_request(
        (
            &"81".repeat(16),
            "127.0.0.1:0",
            &api::encode_hex(space.as_bytes()),
        ),
        (&cert_path, &key_path),
    )?;

    served.refused(&request, ProtocolError::INTERNAL).await?;

    let _written = write_tls_files(&served.state.0)?;
    let retried = call_json(&served, &request).await?;
    assert_eq!(retried.get("error"), None, "{retried}");
    assert_eq!(
        retried.pointer("/result/type"),
        Some(&serde_json::json!("private_relay_configured"))
    );
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
async fn post_commit_and_indeterminate_private_relay_failures_remain_fenced() -> TestResult {
    let served = Served::start().await?;
    let config = StoreConfig::new(&served.state.0);
    let space = served
        .runtime
        .handle()
        .create_owned_space("tls-fenced-failures".to_owned())
        .await?;
    let (cert_path, key_path) = write_tls_files(&served.state.0)?;
    let space_hex = api::encode_hex(space.as_bytes());

    // Post-commit listener bind failure must stay permanently fenced.
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let post_commit_req = private_configure_request(
        (
            &"82".repeat(16),
            &occupied.local_addr()?.to_string(),
            &space_hex,
        ),
        (&cert_path, &key_path),
    )?;
    let post_commit_err = call_json(&served, &post_commit_req).await?;
    assert_eq!(
        post_commit_err
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("internal")
    );
    assert!(
        post_commit_err
            .get("remediation")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| text.contains("mutation committed")
                && text.contains("will not execute again"))
    );
    let post_commit_id = api::decode_command(&serde_json::to_vec(&post_commit_req)?)?
        .request_id()
        .ok_or("missing request id")?;
    assert!(matches!(
        served
            .runtime
            .handle()
            .mutation_replay(post_commit_id)
            .await?,
        Some(MutationReplayState::Completed(_))
    ));
    drop(occupied);
    assert_eq!(call_json(&served, &post_commit_req).await?, post_commit_err);

    // Indeterminate Store commit failure must stay permanently fenced.
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_relay_commit_fence BEFORE UPDATE ON relay_configuration \
         BEGIN SELECT RAISE(ABORT, 'injected relay commit failure'); END;",
    )?;
    let indeterminate_req = private_configure_request(
        (&"83".repeat(16), "127.0.0.1:0", &space_hex),
        (&cert_path, &key_path),
    )?;
    let indeterminate_err = call_json(&served, &indeterminate_req).await?;
    assert_eq!(
        indeterminate_err
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("internal")
    );
    assert!(
        indeterminate_err
            .get("remediation")
            .and_then(serde_json::Value::as_str)
            .is_some_and(
                |text| text.contains("mutation outcome may include side effects")
                    && text.contains("will not execute again")
            )
    );
    let indeterminate_id = api::decode_command(&serde_json::to_vec(&indeterminate_req)?)?
        .request_id()
        .ok_or("missing request id")?;
    assert!(matches!(
        served
            .runtime
            .handle()
            .mutation_replay(indeterminate_id)
            .await?,
        Some(MutationReplayState::Completed(_))
    ));
    sql.execute_batch("DROP TRIGGER test_relay_commit_fence;")?;
    assert_eq!(
        call_json(&served, &indeterminate_req).await?,
        indeterminate_err
    );
    served.stop().await
}
