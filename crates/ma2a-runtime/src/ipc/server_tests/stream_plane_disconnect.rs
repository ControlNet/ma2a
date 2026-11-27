//! Verifies out-of-band IPC peer disconnect when lookahead holds a frame behind a stalled Proxy.

#[cfg(unix)]
use ma2a_core::service::{Channel, FrameKind, MAX_DATA_CHUNK_LEN};
use ma2a_core::service::{
    OpenBody, OpenHeader, ProxyHost, ProxyHostPattern, ProxyRequest, ProxyRule,
};
use ma2a_core::{RequestId, ServiceKind};
use ma2a_store::LocalExposure;

#[cfg(unix)]
use super::stream_plane::{Halves, halves};
use super::{
    TestResult, api,
    stream_plane::{Pair, id},
};
#[cfg(unix)]
use crate::ipc::StreamUpgrade;

#[cfg(windows)]
#[path = "stream_plane_disconnect_windows.rs"]
mod windows;

async fn expose_sink_proxy(
    pair: &Pair,
    request_byte: u8,
    idle_ms: u64,
) -> TestResult<(RequestId, Vec<u8>, tokio::task::JoinHandle<TestResult>)> {
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = tcp_listener.local_addr()?.port();
    let rule = ProxyRule::new(
        ProxyHostPattern::Exact(ProxyHost::new("127.0.0.1")?),
        (port, port),
    )?;
    pair.target
        .handle()
        .expose_service(pair.space, LocalExposure::Proxy(rule))
        .await??;
    let sink_task = tokio::spawn(async move {
        let (_accepted, _) = tcp_listener.accept().await?;
        std::future::pending::<TestResult>().await
    });
    let request_id = id(request_byte)?;
    let request = ProxyRequest::new(ProxyHost::new("127.0.0.1")?, port, idle_ms)?;
    let open = OpenHeader::new(request_id, pair.target_id, OpenBody::Proxy(request)).encode()?;
    Ok((request_id, open, sink_task))
}

fn stream_open_payload(pair: &Pair, request_id: RequestId) -> TestResult<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "operation": "stream_open",
        "request_id": api::encode_hex(request_id.as_bytes()),
        "target_endpoint_id": api::encode_hex(pair.target_id.as_bytes()),
        "service": ServiceKind::Proxy.name(),
    }))?)
}

#[cfg(unix)]
async fn open_backpressured_proxy(
    pair: &Pair,
    request_byte: u8,
    idle_ms: u64,
) -> TestResult<(Halves, tokio::task::JoinHandle<TestResult>)> {
    let (request_id, open, sink_task) = expose_sink_proxy(pair, request_byte, idle_ms).await?;
    let upgrade = api::decode_command(&stream_open_payload(pair, request_id)?)?;
    let StreamUpgrade::Opened(stream) = pair.client.open_stream(&upgrade).await? else {
        return Err("stream_open refused".into());
    };
    let (mut reader, mut writer) = halves(stream);
    writer.frame(FrameKind::Open, &open).await?;
    writer.flush().await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // Flood until local IPC write backpressures so lookahead.ahead holds a frame and more
    // frames remain buffered in the IPC transport.
    let chunk = vec![b'x'; MAX_DATA_CHUNK_LEN];
    let mut sent = 0_usize;
    loop {
        let write = writer.data(Channel::Input, &chunk);
        match tokio::time::timeout(std::time::Duration::from_millis(200), write).await {
            Ok(Ok(())) => sent += 1,
            Ok(Err(error)) => return Err(format!("unexpected write error: {error}").into()),
            Err(_) => break,
        }
        assert!(sent < 512, "proxy upstream never backpressured");
    }
    assert!(
        sent >= 2,
        "expected lookahead plus queued IPC data, sent={sent}"
    );
    Ok(((reader, writer), sink_task))
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_proxy_ipc_client_cancels_within_cancel_grace_even_with_lookahead_buffered()
-> TestResult {
    let pair = Pair::start("stream-a01").await?;
    let ((reader, writer), sink_task) = open_backpressured_proxy(&pair, 0xa1, 60_000).await?;

    // Drop the IPC client without sending Cancel.
    let started = tokio::time::Instant::now();
    drop((reader, writer));

    let cleared = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while !pair
            .caller
            .handle()
            .operations()
            .snapshot()
            .active
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    sink_task.abort();
    assert!(
        cleared.is_ok(),
        "dropped proxy client held operation active past 12 s ({:?})",
        started.elapsed()
    );

    // A live proxy client with lookahead buffered keeps its full idle past CANCEL_GRACE (5 s).
    let ((live_reader, live_writer), live_sink) =
        open_backpressured_proxy(&pair, 0xa2, 8_000).await?;
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        !pair
            .caller
            .handle()
            .operations()
            .snapshot()
            .active
            .is_empty(),
        "live proxy client cancelled early before its 8 s idle"
    );
    drop((live_reader, live_writer));
    live_sink.abort();
    pair.stop().await
}
