//! Frame reader and writer bounds, truncation, and cancel safety.

use ma2a_core::service::{Channel, FrameKind, MAX_DATA_CHUNK_LEN, ServiceError};

use super::{FrameReader, FrameWriter};

#[tokio::test]
async fn frames_roundtrip_and_a_clean_end_is_distinguished_from_truncation() {
    let (client, server) = tokio::io::duplex(1 << 20);
    let mut writer = FrameWriter::new(client);
    writer.data(Channel::Output, b"hello").await.expect("data");
    writer.end(Channel::Input).await.expect("end");
    writer.reject(ServiceError::NotFound).await.expect("reject");
    writer.frame(FrameKind::Cancel, &[]).await.expect("cancel");
    writer.shutdown().await.expect("shutdown");
    let mut reader = FrameReader::new(server);
    let data = reader.expect().await.expect("data frame");
    assert_eq!(
        data.data().expect("payload"),
        (Channel::Output, &b"hello"[..])
    );
    assert_eq!(
        reader.expect().await.expect("end").end(),
        Ok(Channel::Input)
    );
    assert_eq!(
        reader.expect().await.expect("reject").rejection(),
        Ok(ServiceError::NotFound)
    );
    assert_eq!(
        reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );
    assert_eq!(reader.next().await, Ok(None));
}

#[tokio::test]
async fn oversized_and_truncated_frames_are_refused_before_buffering() {
    let (mut client, server) = tokio::io::duplex(64);
    tokio::io::AsyncWriteExt::write_all(&mut client, &[4, 0, 0x10, 0, 2])
        .await
        .expect("header");
    drop(client);
    assert_eq!(
        FrameReader::new(server).next().await,
        Err(ServiceError::InvalidInput)
    );
    let (mut client, server) = tokio::io::duplex(64);
    tokio::io::AsyncWriteExt::write_all(&mut client, &[5, 0, 0, 0, 1])
        .await
        .expect("header");
    drop(client);
    assert_eq!(
        FrameReader::new(server).next().await,
        Err(ServiceError::Cancelled)
    );
    let (client, _server) = tokio::io::duplex(64);
    let oversized = vec![1_u8; MAX_DATA_CHUNK_LEN + 1];
    assert_eq!(
        FrameWriter::new(client)
            .data(Channel::Input, &oversized)
            .await,
        Err(ServiceError::InvalidInput)
    );
}

#[tokio::test]
async fn a_read_abandoned_mid_frame_resumes_without_losing_bytes() {
    let mut framer = FrameWriter::new(Vec::new());
    framer
        .data(Channel::Output, b"split frame")
        .await
        .expect("encode");
    let wire = framer.into_inner();
    let (mut client, server) = tokio::io::duplex(1 << 16);
    let mut reader = FrameReader::new(server);
    // One split inside the header and one inside the payload.
    for split in [2, 8] {
        let (head, tail) = wire.split_at(split);
        tokio::io::AsyncWriteExt::write_all(&mut client, head)
            .await
            .expect("head");
        let abandoned =
            tokio::time::timeout(std::time::Duration::from_millis(20), reader.next()).await;
        assert!(abandoned.is_err(), "a partial frame is never returned");
        tokio::io::AsyncWriteExt::write_all(&mut client, tail)
            .await
            .expect("tail");
        let frame = reader.expect().await.expect("resumed frame");
        assert_eq!(
            frame.data().expect("payload"),
            (Channel::Output, &b"split frame"[..])
        );
    }
}

#[tokio::test]
async fn a_frame_abandoned_mid_write_refuses_every_later_frame() {
    // Given: a peer that reads nothing, so a large data frame stalls after its first bytes.
    let (client, _server) = tokio::io::duplex(64);
    let mut writer = FrameWriter::new(client);
    let chunk = vec![7_u8; MAX_DATA_CHUNK_LEN];
    let stalled = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        writer.data(Channel::Output, &chunk),
    )
    .await;
    assert!(stalled.is_err(), "the write must have stalled");

    // When: an outcome follows the abandoned frame on the same writer.
    let outcome = writer.frame(FrameKind::Outcome, &[0]).await;

    // Then: it is refused rather than read by the peer as the rest of the data frame.
    assert!(writer.is_torn());
    assert_eq!(outcome, Err(ServiceError::Cancelled));
}

struct FailsAfterWrite {
    remaining: usize,
    fail_flush: bool,
}

impl tokio::io::AsyncWrite for FailsAfterWrite {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.remaining == 0 {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let take = buffer.len().min(self.remaining);
        self.remaining -= take;
        std::task::Poll::Ready(Ok(take))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.fail_flush {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        } else {
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn write_open_keeps_cancelled_only_before_any_byte_is_written() {
    // Out-of-bound header fails before any byte is written.
    let mut writer = FrameWriter::new(Vec::new());
    assert_eq!(
        writer.write_open(&[]).await,
        Err(ServiceError::InvalidInput)
    );
    assert!(!writer.has_written());

    // Write failure before byte 0: nothing could have reached the target -> Cancelled.
    let mut writer = FrameWriter::new(FailsAfterWrite {
        remaining: 0,
        fail_flush: false,
    });
    assert_eq!(
        writer.write_open(b"open-header").await,
        Err(ServiceError::Cancelled)
    );
    assert!(!writer.has_written());

    // Partial header (2 B), full frame header before payload (5 B), partial payload (7 B),
    // and flush failure after full write: bytes may have reached the target -> OutcomeUnknown.
    for (remaining, fail_flush) in [(2, false), (5, false), (7, false), (usize::MAX, true)] {
        let mut writer = FrameWriter::new(FailsAfterWrite {
            remaining,
            fail_flush,
        });
        assert_eq!(
            writer.write_open(b"open-header").await,
            Err(ServiceError::OutcomeUnknown),
            "remaining={remaining}, fail_flush={fail_flush}"
        );
        assert!(writer.has_written());
    }
}
