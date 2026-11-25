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
