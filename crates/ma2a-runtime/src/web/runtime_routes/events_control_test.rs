use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tokio_stream::StreamExt as _;
use tower::ServiceExt as _;

async fn fetch_snapshot(
    router: &Router,
    cookie: &str,
) -> Result<(String, serde_json::Value), Box<dyn std::error::Error + Send + Sync>> {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43212")
                .header("cookie", cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let boot_id = response
        .headers()
        .get("x-ma2a-runtime-boot-id")
        .ok_or("missing boot id header")?
        .to_str()?
        .to_owned();
    let payload = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    Ok((boot_id, payload))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_control_round_invalidates_sse_and_updates_snapshot_history()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_state, runtime, cancel, task, paths, router, cookie) =
        super::tests::start_test_router().await?;

    // Let any startup round settle before capturing the baseline snapshot.
    runtime.handle().sync_control().await?;

    let (boot_id, before) = fetch_snapshot(&router, &cookie).await?;
    let before_revision = before
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing before revision")?;
    let before_rounds = before
        .get("control_rounds")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing before control_rounds")?
        .len();

    let sse_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/events?since={before_revision}&boot_id={boot_id}"
                ))
                .header("host", "127.0.0.1:43212")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let mut sse_body = sse_resp.into_body().into_data_stream();

    // Trigger an empty control round (no peers configured).
    runtime.handle().sync_control().await?;

    let event = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match sse_body.next().await {
                Some(Ok(chunk)) => {
                    let text = String::from_utf8_lossy(&chunk);
                    if text.contains("snapshot_invalidated") || text.contains("resync-required") {
                        return Ok::<String, String>(text.into_owned());
                    }
                }
                Some(Err(error)) => return Err(format!("sse error: {error}")),
                None => return Err("sse stream closed without invalidation".to_owned()),
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for SSE invalidation after empty control round")??;
    assert!(
        event.contains("snapshot_invalidated") || event.contains("resync-required"),
        "expected SSE invalidation, got: {event}"
    );
    drop(sse_body);

    let (_boot_id, after) = fetch_snapshot(&router, &cookie).await?;
    let after_revision = after
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing after revision")?;
    let after_rounds = after
        .get("control_rounds")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing after control_rounds")?;

    assert!(
        after_revision > before_revision,
        "expected revision to advance ({before_revision} -> {after_revision})"
    );
    assert_eq!(after_rounds.len(), before_rounds + 1);
    assert_eq!(
        after_rounds.last().and_then(|round| round.get("outcome")),
        Some(&serde_json::json!("empty"))
    );

    cancel.cancel();
    task.await??;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    Ok(())
}
