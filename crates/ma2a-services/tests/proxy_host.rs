//! Proxy host byte-exact relay, half-close, connect failures, idle timeout, and cancel.

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, ProxyHost, ProxyRequest, ProxyResult,
    ServiceError,
};
use ma2a_services::{Frame, OperationEnd, OperationReport, serve_proxy};
use support::{ClientIo, TestResult, content, drain, pipe_with};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

fn start(request: ProxyRequest) -> (ClientIo, tokio::task::JoinHandle<OperationReport>) {
    let (client, (mut reader, mut writer)) = pipe_with(1 << 20);
    let server = tokio::spawn(async move {
        serve_proxy(
            request,
            &CancellationToken::new(),
            (&mut reader, &mut writer),
        )
        .await
    });
    (client, server)
}

fn loopback(port: u16, idle_ms: u64) -> TestResult<ProxyRequest> {
    Ok(ProxyRequest::new(
        ProxyHost::new("127.0.0.1")?,
        port,
        idle_ms,
    )?)
}

fn relayed(frames: &[Frame]) -> TestResult<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut ended = false;
    for frame in frames {
        match frame.kind() {
            FrameKind::Data => {
                let (channel, chunk) = frame.data()?;
                assert_eq!(channel, Channel::Input);
                assert!(!ended, "data after end");
                bytes.extend_from_slice(chunk);
            }
            FrameKind::End => ended = true,
            _ => {}
        }
    }
    Ok((bytes, ended))
}

fn outcome(frames: &[Frame]) -> TestResult<Result<ProxyResult, ServiceError>> {
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    Ok(ProxyResult::decode_outcome(last.payload())?)
}

#[tokio::test]
async fn bytes_are_relayed_exactly_and_half_close_propagates() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let echo = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut received = Vec::new();
        socket.read_to_end(&mut received).await?;
        socket.write_all(&received).await?;
        socket.write_all(b"|eof-seen").await?;
        socket.shutdown().await?;
        Ok::<usize, std::io::Error>(received.len())
    });
    let payload = content(1 << 20);
    let ((mut reader, mut writer), server) = start(loopback(port, 10_000)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    for chunk in payload.chunks(MAX_DATA_CHUNK_LEN) {
        writer.data(Channel::Input, chunk).await?;
    }
    writer.end(Channel::Input).await?;
    let frames = drain(&mut reader).await?;
    assert_eq!(echo.await??, payload.len());
    let (bytes, ended) = relayed(&frames)?;
    assert!(ended);
    let mut expected = payload.clone();
    expected.extend_from_slice(b"|eof-seen");
    assert_eq!(bytes, expected);
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.reason, EndReason::Completed);
    assert_eq!(result.bytes_to_target, 1 << 20);
    assert_eq!(result.bytes_from_target, (1 << 20) + 9);
    assert_eq!(server.await?.end, OperationEnd::Completed);
    Ok(())
}

#[tokio::test]
async fn unreachable_destinations_are_rejected_before_accept() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let ((mut reader, _writer), server) = start(loopback(port, 1_000)?);
    let frames = drain(&mut reader).await?;
    assert_eq!(
        frames.first().ok_or("reject")?.rejection()?,
        ServiceError::ConnectionRefused
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Rejected(ServiceError::ConnectionRefused)
    );
    let request = ProxyRequest::new(ProxyHost::new("ma2a-test.invalid")?, 80, 1_000)?;
    let ((mut reader, _writer), server) = start(request);
    let frames = tokio::time::timeout(Duration::from_secs(20), drain(&mut reader)).await??;
    let error = frames.first().ok_or("reject")?.rejection()?;
    assert!(
        matches!(error, ServiceError::DnsFailed | ServiceError::TimedOut),
        "{error:?}"
    );
    assert!(matches!(server.await?.end, OperationEnd::Rejected(_)));
    Ok(())
}

#[tokio::test]
async fn idle_streams_time_out_and_cancel_ends_the_relay() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let holder = tokio::spawn(async move {
        let mut held = Vec::new();
        for _ in 0..2 {
            held.push(listener.accept().await?.0);
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok::<_, std::io::Error>(held.len())
    });
    let ((mut reader, _writer), server) = start(loopback(port, 200)?);
    let frames = tokio::time::timeout(Duration::from_secs(10), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.reason, EndReason::TimedOut);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::TimedOut)
    );

    let ((mut reader, mut writer), server) = start(loopback(port, 10_000)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"partial").await?;
    writer.frame(FrameKind::Cancel, &[]).await?;
    let frames = tokio::time::timeout(Duration::from_secs(10), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        (result.reason, result.bytes_to_target),
        (EndReason::Cancelled, 7)
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    holder.abort();
    Ok(())
}

#[tokio::test]
async fn a_write_blocked_for_the_idle_timeout_ends_the_relay_while_the_destination_talks()
-> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    // Never reads, but keeps sending one byte at a time, which counts as activity.
    let talker = tokio::spawn(async move {
        let mut socket = listener.accept().await?.0;
        while socket.write_all(b".").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<_, std::io::Error>(socket)
    });
    let ((mut reader, mut writer), server) = start(loopback(port, 300)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    // Far more than the loopback socket buffers hold, so the write to the destination blocks.
    let flood = tokio::spawn(async move {
        let chunk = content(MAX_DATA_CHUNK_LEN);
        for _ in 0..1024 {
            if writer.data(Channel::Input, &chunk).await.is_err() {
                break;
            }
        }
    });
    let frames = tokio::time::timeout(Duration::from_secs(10), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.reason, EndReason::TimedOut);
    assert!(result.bytes_from_target > 0, "the destination never talked");
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::TimedOut)
    );
    flood.abort();
    talker.abort();
    Ok(())
}

#[tokio::test]
async fn a_cancel_after_end_stops_a_relay_to_a_silent_destination() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let holder = tokio::spawn(async move {
        let held = listener.accept().await?.0;
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok::<_, std::io::Error>(held)
    });
    let ((mut reader, mut writer), server) = start(loopback(port, 60_000)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"request").await?;
    writer.end(Channel::Input).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    writer.frame(FrameKind::Cancel, &[]).await?;
    let frames = tokio::time::timeout(Duration::from_secs(5), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    holder.abort();
    Ok(())
}

#[tokio::test]
async fn a_caller_closing_before_end_cancels_the_relay() -> TestResult {
    // Given: an EOF-delimited destination that replies once its input closes.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let echo = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut received = Vec::new();
        let _read = socket.read_to_end(&mut received).await;
        let _wrote = socket.write_all(b"done").await;
        let _closed = socket.shutdown().await;
        Ok::<Vec<u8>, std::io::Error>(received)
    });
    let ((mut reader, mut writer), server) = start(loopback(port, 10_000)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"part").await?;

    // When: the caller closes its transport write half without sending End(Input).
    writer.shutdown().await?;

    // Then: truncated input is never treated as completed; the relay cancels.
    let frames = tokio::time::timeout(Duration::from_secs(2), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        (
            result.reason,
            result.bytes_to_target,
            result.bytes_from_target
        ),
        (EndReason::Cancelled, 4, 0)
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    let _done = echo.await;
    Ok(())
}

#[tokio::test]
async fn a_caller_closing_after_end_cancels_a_silent_destination() -> TestResult {
    // Given: a silent destination with a long idle timeout.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let holder = tokio::spawn(async move {
        let held = listener.accept().await?.0;
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok::<_, std::io::Error>(held)
    });
    let ((mut reader, mut writer), server) = start(loopback(port, 60_000)?);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"request").await?;
    writer.end(Channel::Input).await?;

    // When: the caller closes its write half while the destination is still live.
    writer.shutdown().await?;

    // Then: the relay cancels at once instead of waiting for the idle timeout.
    let frames = tokio::time::timeout(Duration::from_secs(2), drain(&mut reader)).await??;
    let result = outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        (
            result.reason,
            result.bytes_to_target,
            result.bytes_from_target
        ),
        (EndReason::Cancelled, 7, 0)
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    holder.abort();
    Ok(())
}

#[tokio::test]
async fn a_caller_that_never_reads_the_accept_does_not_hold_the_proxy() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let target = tokio::spawn(async move { listener.accept().await });
    // No room for even the accept header, and the caller never reads.
    let (_client, (mut reader, mut writer)) = pipe_with(1);
    let shutdown = CancellationToken::new();
    let server = {
        let shutdown = shutdown.clone();
        let request = loopback(port, 10_000)?;
        tokio::spawn(
            async move { serve_proxy(request, &shutdown, (&mut reader, &mut writer)).await },
        )
    };
    let _connected = target.await??;
    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown.cancel();
    let report = tokio::time::timeout(Duration::from_secs(3), server).await??;
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::Unavailable));
    Ok(())
}
