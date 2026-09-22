use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use super::{IO_DEADLINE, IpcError};

const HEADER_BYTES: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) correlation: u64,
    pub(crate) payload: Vec<u8>,
}

pub(crate) struct FrameRef<'a> {
    pub(crate) correlation: u64,
    pub(crate) payload: &'a [u8],
    pub(crate) maximum: usize,
}

pub(crate) async fn read_frame(
    stream: &mut (impl AsyncRead + Unpin),
    maximum: usize,
) -> Result<Frame, IpcError> {
    let header = read_header(stream, IO_DEADLINE, IpcError::InvalidFrame).await?;
    read_payload(stream, maximum, header).await
}

/// Reads one response frame, allowing `arrival` for the Runtime to start answering.
///
/// Waiting for a reply is a command property; transferring a frame already in
/// flight is a transport property. They use different deadlines and different
/// errors, so a Runtime still executing a valid command is never reported as
/// malformed framing.
pub(crate) async fn read_frame_within(
    stream: &mut (impl AsyncRead + Unpin),
    maximum: usize,
    arrival: Duration,
) -> Result<Frame, IpcError> {
    let header = read_header(stream, arrival, IpcError::ResponseTimeout).await?;
    read_payload(stream, maximum, header).await
}

async fn read_header(
    stream: &mut (impl AsyncRead + Unpin),
    arrival: Duration,
    stalled: IpcError,
) -> Result<[u8; HEADER_BYTES], IpcError> {
    let mut header = [0_u8; HEADER_BYTES];
    let (first, remainder) = header.split_at_mut(1);
    tokio::time::timeout(arrival, stream.read_exact(first))
        .await
        .map_err(|_| stalled)??;
    // Once the peer has begun a frame, the rest of it is pure transport.
    tokio::time::timeout(IO_DEADLINE, stream.read_exact(remainder))
        .await
        .map_err(|_| IpcError::InvalidFrame)??;
    Ok(header)
}

async fn read_payload(
    stream: &mut (impl AsyncRead + Unpin),
    maximum: usize,
    header: [u8; HEADER_BYTES],
) -> Result<Frame, IpcError> {
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let length = usize::try_from(length).map_err(|_| IpcError::InvalidFrame)?;
    if length == 0 || length > maximum {
        return Err(IpcError::InvalidFrame);
    }
    let correlation = u64::from_be_bytes([
        header[4], header[5], header[6], header[7], header[8], header[9], header[10], header[11],
    ]);
    let mut payload = vec![0_u8; length];
    tokio::time::timeout(IO_DEADLINE, stream.read_exact(&mut payload))
        .await
        .map_err(|_| IpcError::InvalidFrame)??;
    Ok(Frame {
        correlation,
        payload,
    })
}

pub(crate) async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    frame: FrameRef<'_>,
) -> Result<(), IpcError> {
    if frame.payload.is_empty() || frame.payload.len() > frame.maximum {
        return Err(IpcError::InvalidFrame);
    }
    let length = u32::try_from(frame.payload.len()).map_err(|_| IpcError::InvalidFrame)?;
    let mut header = [0_u8; HEADER_BYTES];
    header[..4].copy_from_slice(&length.to_be_bytes());
    header[4..].copy_from_slice(&frame.correlation.to_be_bytes());
    tokio::time::timeout(IO_DEADLINE, async {
        stream.write_all(&header).await?;
        stream.write_all(frame.payload).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| IpcError::InvalidFrame)??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncWriteExt as _, duplex};

    use super::*;

    #[tokio::test]
    async fn rejects_oversized_frame_before_payload_read() {
        // Given
        let (mut writer, mut reader) = duplex(64);
        writer
            .write_all(&65_537_u32.to_be_bytes())
            .await
            .expect("header write");
        writer
            .write_all(&7_u64.to_be_bytes())
            .await
            .expect("correlation write");

        // When
        let result = read_frame(&mut reader, 65_536).await;

        // Then
        assert!(matches!(result, Err(IpcError::InvalidFrame)));
    }

    #[tokio::test(start_paused = true)]
    async fn rejects_slow_partial_frame_at_deadline() {
        // Given
        let (_writer, mut reader) = duplex(64);

        // When
        let task = tokio::spawn(async move { read_frame(&mut reader, 16_384).await });
        tokio::time::advance(IO_DEADLINE).await;
        let result = task.await.expect("reader task");

        // Then
        assert!(matches!(result, Err(IpcError::InvalidFrame)));
    }

    /// A Runtime that is still doing valid work has not sent a malformed frame,
    /// so the client must keep waiting for its own command deadline.
    #[tokio::test(start_paused = true)]
    async fn a_reply_after_the_transport_deadline_still_arrives() {
        // Given
        let (mut writer, mut reader) = duplex(256);
        let arrival = IO_DEADLINE * 8;

        // When
        let task =
            tokio::spawn(async move { read_frame_within(&mut reader, 16_384, arrival).await });
        tokio::time::advance(IO_DEADLINE * 4).await;
        write_frame(
            &mut writer,
            FrameRef {
                correlation: 9,
                payload: b"late",
                maximum: 16_384,
            },
        )
        .await
        .expect("delayed reply");

        // Then
        let frame = task.await.expect("reader task").expect("delayed frame");
        assert_eq!(frame.correlation, 9);
        assert_eq!(frame.payload, b"late");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_runtime_reports_a_command_timeout_not_a_broken_frame() {
        // Given
        let (_writer, mut reader) = duplex(64);
        let arrival = IO_DEADLINE * 4;

        // When
        let task =
            tokio::spawn(async move { read_frame_within(&mut reader, 16_384, arrival).await });
        tokio::time::advance(arrival).await;
        let result = task.await.expect("reader task");

        // Then
        assert!(
            matches!(result, Err(IpcError::ResponseTimeout)),
            "{result:?}"
        );
    }

    /// A frame that starts and then stalls is malformed even when the command
    /// itself was allowed a long deadline.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_frame_body_remains_bounded_by_the_transport_deadline() {
        // Given
        let (mut writer, mut reader) = duplex(64);
        let arrival = IO_DEADLINE * 30;
        writer
            .write_all(&4_u32.to_be_bytes())
            .await
            .expect("length");

        // When
        let task =
            tokio::spawn(async move { read_frame_within(&mut reader, 16_384, arrival).await });
        tokio::time::advance(IO_DEADLINE * 2).await;
        let result = task.await.expect("reader task");

        // Then
        assert!(matches!(result, Err(IpcError::InvalidFrame)), "{result:?}");
    }
}
