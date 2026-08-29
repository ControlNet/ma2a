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
    let mut header = [0_u8; HEADER_BYTES];
    tokio::time::timeout(IO_DEADLINE, stream.read_exact(&mut header))
        .await
        .map_err(|_| IpcError::InvalidFrame)??;
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
}
