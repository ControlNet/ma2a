use std::time::Duration;

use serde_json::{Value, json};

use super::services::{Harness, call, text};
use super::*;

const MANIFEST: &str = r#"{"description":"An example.","max_calls":2,"versions":[{"version":1,"operations":[{"name":"echo","interaction":"unary"}]},{"version":2,"operations":[{"name":"echo","interaction":"unary"},{"name":"count","interaction":"stream"}]}]}"#;

fn named(operation: &str, request_id: &str) -> Value {
    json!({"version": 1, "operation": operation, "request_id": request_id, "service": "example"})
}

fn register(request_id: &str) -> TestResult<Value> {
    Ok(json!({
        "version": 1, "operation": "service_register", "request_id": request_id,
        "service": "example", "manifest": serde_json::from_str::<Value>(MANIFEST)?,
    }))
}

/// Upgrades one connection with a hosted command; the answer, and the stream if upgraded.
async fn upgrade(
    client: &LocalApiClient,
    request: Value,
) -> TestResult<(Value, Option<crate::ipc::LocalStream>)> {
    let command = api::decode_command(&serde_json::to_vec(&request)?)?;
    let (response, stream) = client.open_hosted(&command).await?;
    Ok((serde_json::from_slice(&response)?, stream))
}

fn attach() -> Value {
    json!({"version": 1, "operation": "service_attach", "service": "example"})
}

/// Waits until the one registration reports `state`.
async fn wait_for_state(client: &LocalApiClient, state: &str) -> TestResult<Value> {
    let list = json!({"version": 1, "operation": "service_hosted_list", "after": null});
    for _ in 0..100 {
        let listed = call(client, list.clone()).await?;
        if text(&listed, "/result/payload/services/0/state") == Some(state) {
            return Ok(listed);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(format!("the service never became {state}").into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_service_links_one_process_until_it_is_stopped() -> TestResult {
    let harness = Harness::start("hosted-link").await?;
    let client = &harness.client;

    let registered = call(client, register(&"31".repeat(16))?).await?;
    assert_eq!(
        text(&registered, "/result/type"),
        Some("service_registered")
    );
    assert_eq!(
        registered.pointer("/result/payload/created"),
        Some(&json!(true))
    );
    assert_eq!(
        text(&registered, "/result/payload/service/state"),
        Some("stopped")
    );
    let (refused, stream) = upgrade(client, attach()).await?;
    assert!(stream.is_none());
    assert_eq!(text(&refused, "/error"), Some("unavailable"));

    let started = call(client, named("service_start", &"32".repeat(16))).await?;
    assert_eq!(text(&started, "/result/payload/state"), Some("unavailable"));
    let (attached, stream) = upgrade(client, attach()).await?;
    assert_eq!(text(&attached, "/result/type"), Some("service_attached"));
    assert_eq!(
        text(
            &attached,
            "/result/payload/manifest/versions/1/operations/1/name"
        ),
        Some("count")
    );
    let (mut notices, mut reports) = stream.ok_or("attach did not upgrade")?.into_link();
    let (second, none) = upgrade(client, attach()).await?;
    assert!(none.is_none());
    assert_eq!(text(&second, "/error"), Some("conflict"));

    reports.send(br#"{"t":"ready"}"#).await?;
    let ready = wait_for_state(client, "ready").await?;
    assert_eq!(
        ready.pointer("/result/payload/services/0/running_calls"),
        Some(&json!(0))
    );
    let replaced = call(client, register(&"33".repeat(16))?).await?;
    assert_eq!(text(&replaced, "/error"), Some("conflict"));
    reports.send(br#"{"t":"unavailable"}"#).await?;
    wait_for_state(client, "unavailable").await?;

    let stopped = call(client, named("service_stop", &"34".repeat(16))).await?;
    assert_eq!(text(&stopped, "/result/payload/state"), Some("stopped"));
    let stop: Value = serde_json::from_slice(&notices.next().await?)?;
    assert_eq!(stop, json!({"t": "stop"}));
    assert!(notices.next().await.is_err(), "the link ends after stop");

    let removed = call(client, named("service_unregister", &"35".repeat(16))).await?;
    assert_eq!(text(&removed, "/result/payload/service"), Some("example"));
    let missing = call(client, named("service_start", &"36".repeat(16))).await?;
    assert_eq!(text(&missing, "/error"), Some("not_found"));
    harness.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_link_makes_the_service_unavailable_and_frees_it_for_a_new_process() -> TestResult {
    let harness = Harness::start("hosted-lost").await?;
    let client = &harness.client;
    call(client, register(&"41".repeat(16))?).await?;
    call(client, named("service_start", &"42".repeat(16))).await?;

    let (_, stream) = upgrade(client, attach()).await?;
    let (notices, mut reports) = stream.ok_or("attach did not upgrade")?.into_link();
    reports.send(br#"{"t":"ready"}"#).await?;
    wait_for_state(client, "ready").await?;
    drop((notices, reports));
    wait_for_state(client, "unavailable").await?;

    let (again, stream) = upgrade(client, attach()).await?;
    assert_eq!(text(&again, "/result/type"), Some("service_attached"));
    let (_notices, mut reports) = stream.ok_or("re-attach did not upgrade")?.into_link();
    // Anything but a report ends the link.
    reports.send(br#"{"t":"ready","extra":1}"#).await?;
    wait_for_state(client, "unavailable").await?;

    let unknown = json!({
        "version": 1, "operation": "service_accept", "service": "example",
        "call_id": "00".repeat(16),
    });
    let (refused, none) = upgrade(client, unknown).await?;
    assert!(none.is_none());
    assert_eq!(text(&refused, "/error"), Some("not_found"));
    harness.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_registration_starts_its_call_counts_afresh() -> TestResult {
    // Given: version 2 of the first registration served a call.
    let harness = Harness::start("hosted-recount").await?;
    let client = &harness.client;
    call(client, register(&"51".repeat(16))?).await?;
    let name = ma2a_core::service::ServiceName::new("example")?;
    harness.hosted()?.count_call(&name, 2);
    let list = json!({"version": 1, "operation": "service_hosted_list", "after": null});
    let counted = call(client, list.clone()).await?;
    assert_eq!(
        counted.pointer("/result/payload/services/0/calls"),
        Some(&json!([{"version": 2, "count": 1}]))
    );

    // When: the stopped service is registered again with only version 1.
    let mut replacement = register(&"52".repeat(16))?;
    if let Some(versions) = replacement
        .pointer_mut("/manifest/versions")
        .and_then(Value::as_array_mut)
    {
        versions.truncate(1);
    }
    let replaced = call(client, replacement).await?;

    // Then: no count of the old registration remains.
    assert_eq!(
        replaced.pointer("/result/payload/created"),
        Some(&json!(false))
    );
    assert_eq!(
        replaced.pointer("/result/payload/service/calls"),
        Some(&json!([]))
    );
    let listed = call(client, list).await?;
    assert_eq!(
        listed.pointer("/result/payload/services/0/calls"),
        Some(&json!([]))
    );
    harness.stop().await
}
