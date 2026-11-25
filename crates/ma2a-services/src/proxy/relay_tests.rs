//! Proxy relay blocked-write cancellation and stream-close behavior.

use std::time::Duration;

use ma2a_core::service::{Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN};
use tokio::net::{TcpSocket, TcpStream};
use tokio_util::sync::CancellationToken;

use super::{relay_streams, run};
use crate::{FrameReader, FrameWriter};

async fn tiny_loopback() -> Result<(TcpStream, TcpStream), std::io::Error> {
    let listener = TcpSocket::new_v4()?;
    listener.set_recv_buffer_size(4096)?;
    listener.bind("127.0.0.1:0".parse().expect("loopback"))?;
    let listener = listener.listen(1)?;
    let addr = listener.local_addr()?;
    let client = TcpSocket::new_v4()?;
    client.set_send_buffer_size(4096)?;
    let (stream, accepted) = tokio::try_join!(client.connect(addr), async {
        listener.accept().await.map(|(socket, _)| socket)
    })?;
    Ok((stream, accepted))
}

#[tokio::test(start_paused = true)]
async fn a_blocked_write_keeps_its_idle_timeout_with_data_ahead_and_drains_in_order()
-> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // Case 1: another data frame queued behind a blocked write ends as TimedOut at `idle`
    // (40 s, past the 30 s Exec/Shell input stall bound), not Cancelled.
    let (client, server) = tokio::io::duplex(1 << 16);
    let ((_client_read, client_write), (server_read, server_write)) =
        (tokio::io::split(client), tokio::io::split(server));
    let mut caller = FrameWriter::new(client_write);
    let (mut reader, mut writer) = (
        FrameReader::new(server_read),
        FrameWriter::new(server_write),
    );
    let (target, _silent) = tokio::io::duplex(1);
    let (target_read, target_write) = tokio::io::split(target);
    let shutdown = CancellationToken::new();
    let idle = Duration::from_secs(40);
    let started = tokio::time::Instant::now();
    let relayed = tokio::spawn(async move {
        relay_streams(
            (target_read, target_write),
            (&mut reader, &mut writer),
            (idle, &shutdown),
        )
        .await
    });
    caller.data(Channel::Input, b"ab").await?;
    caller.data(Channel::Input, b"cd").await?;
    caller.flush().await?;
    let result = relayed.await?;
    assert_eq!(result.result, Ok(EndReason::TimedOut));
    assert_eq!(result.to_target, 1);
    assert_eq!(started.elapsed(), idle);

    // Case 2: a destination that starts reading within `idle` receives the blocked bytes,
    // the held data frame, and the trailing End(Input) in exact order.
    let (client, server) = tokio::io::duplex(1 << 16);
    let ((client_read, client_write), (server_read, server_write)) =
        (tokio::io::split(client), tokio::io::split(server));
    let mut caller = FrameWriter::new(client_write);
    let mut client_reader = FrameReader::new(client_read);
    let (mut reader, mut writer) = (
        FrameReader::new(server_read),
        FrameWriter::new(server_write),
    );
    let (target, mut destination) = tokio::io::duplex(1);
    let (target_read, target_write) = tokio::io::split(target);
    let shutdown = CancellationToken::new();
    let relayed = tokio::spawn(async move {
        relay_streams(
            (target_read, target_write),
            (&mut reader, &mut writer),
            (idle, &shutdown),
        )
        .await
    });
    caller.data(Channel::Input, b"ab").await?;
    caller.data(Channel::Input, b"cd").await?;
    caller.end(Channel::Input).await?;
    caller.flush().await?;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut received = Vec::new();
    destination.read_to_end(&mut received).await?;
    assert_eq!(received, b"abcd");
    destination.shutdown().await?;
    assert_eq!(client_reader.expect().await?.end(), Ok(Channel::Input));
    let result = relayed.await?;
    assert_eq!(result.result, Ok(EndReason::Completed));
    assert_eq!(result.to_target, 4);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum StopCase {
    Cancel,
    EndThenCancel,
    Eof,
}

#[tokio::test]
async fn a_cancel_or_close_behind_a_blocked_destination_write_ends_the_relay_at_once()
-> Result<(), Box<dyn std::error::Error>> {
    // Cover direct Cancel, End(Input) followed by Cancel, and early transport EOF behind a
    // destination write blocked on full TCP buffers.
    for case in [StopCase::Cancel, StopCase::EndThenCancel, StopCase::Eof] {
        let (stream, _silent_target) = tiny_loopback().await?;
        let (client, server) = tokio::io::duplex(1 << 18);
        let ((_client_read, client_write), (server_read, server_write)) =
            (tokio::io::split(client), tokio::io::split(server));
        let mut caller = FrameWriter::new(client_write);
        let (mut reader, mut writer) = (
            FrameReader::new(server_read),
            FrameWriter::new(server_write),
        );
        let shutdown = CancellationToken::new();
        let relayed = tokio::spawn(async move {
            run(
                stream,
                (&mut reader, &mut writer),
                (Duration::from_secs(60), &shutdown),
            )
            .await
        });

        // One 64 KiB frame exceeds the 4 KiB send+recv TCP buffers, so `target.write` blocks.
        let chunk = vec![b'x'; MAX_DATA_CHUNK_LEN];
        caller.data(Channel::Input, &chunk).await?;
        match case {
            StopCase::Cancel => caller.frame(FrameKind::Cancel, &[]).await?,
            StopCase::EndThenCancel => {
                caller.end(Channel::Input).await?;
                caller.frame(FrameKind::Cancel, &[]).await?;
            }
            StopCase::Eof => caller.shutdown().await?,
        }

        let relayed = tokio::time::timeout(Duration::from_secs(2), relayed).await??;
        assert_eq!(relayed.result, Ok(EndReason::Cancelled), "{case:?}");
        assert!(
            relayed.to_target < u64::try_from(MAX_DATA_CHUNK_LEN).unwrap_or(u64::MAX),
            "{case:?}: write did not block (wrote {} bytes)",
            relayed.to_target
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stalled_caller_exhausts_frame_grace_while_a_slow_caller_gets_a_whole_frame()
-> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncWriteExt as _;

    // Slow caller: shutdown fires while `downstream` is blocked mid-frame on a 1-byte pipe;
    // draining 1 s later finishes that single frame intact with matching byte counts.
    let (client, server) = tokio::io::duplex(1);
    let ((client_read, _client_write), (server_read, server_write)) =
        (tokio::io::split(client), tokio::io::split(server));
    let mut client_reader = FrameReader::new(client_read);
    let (mut reader, mut writer) = (
        FrameReader::new(server_read),
        FrameWriter::new(server_write),
    );
    let (target, mut destination) = tokio::io::duplex(64);
    let (target_read, target_write) = tokio::io::split(target);
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let relayed = tokio::spawn(async move {
        let result = relay_streams(
            (target_read, target_write),
            (&mut reader, &mut writer),
            (Duration::from_secs(60), &stop),
        )
        .await;
        (result, writer.is_torn())
    });
    destination.write_all(b"hello").await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    shutdown.cancel();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let frame = client_reader.expect().await?;
    assert_eq!(frame.data()?, (Channel::Input, b"hello".as_slice()));
    let (result, torn) = relayed.await?;
    assert!(!torn);
    assert_eq!(result.result, Ok(EndReason::Shutdown));
    assert_eq!(result.from_target, 5);

    // Stalled caller: never reads at all; relay gives up after FRAME_GRACE and leaves writer torn.
    let (_client, server) = tokio::io::duplex(1);
    let (server_read, server_write) = tokio::io::split(server);
    let (mut reader, mut writer) = (
        FrameReader::new(server_read),
        FrameWriter::new(server_write),
    );
    let (target, mut destination) = tokio::io::duplex(64);
    let (target_read, target_write) = tokio::io::split(target);
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let relayed = tokio::spawn(async move {
        let result = relay_streams(
            (target_read, target_write),
            (&mut reader, &mut writer),
            (Duration::from_secs(60), &stop),
        )
        .await;
        (result, writer.is_torn())
    });
    destination.write_all(b"hello").await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let stopped_at = tokio::time::Instant::now();
    shutdown.cancel();
    let (result, torn) = relayed.await?;
    assert!(torn);
    assert_eq!(result.result, Ok(EndReason::Shutdown));
    assert_eq!(result.from_target, 0);
    assert_eq!(stopped_at.elapsed(), crate::reply::FRAME_GRACE);
    Ok(())
}
