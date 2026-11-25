//! Stdin writes: channel 0 toward the Process until the caller's EOF.

use ma2a_core::service::{Channel, FrameKind, MAX_DATA_CHUNK_LEN, ProcessReply, ServiceError};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};

use super::reply;
use crate::{
    FrameReader, FrameWriter,
    client::{ClientError, First, first, next, unexpected},
};

/// Writes local input to the Process stdin until EOF, then reports what was delivered.
///
/// Returns the bytes the Process received and whether its stdin is now closed.
///
/// # Errors
/// Returns the refusal (`writer_busy`, `stdin_closed`, `not_running`), the failed outcome,
/// or the stream failure.
pub async fn stdin<R, W, I>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    input: I,
) -> Result<(u64, bool), ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
{
    let frame = match first(reader).await? {
        First::Finished(frame) => frame,
        First::Accepted(_) => {
            let sent = send_all(writer, input);
            tokio::pin!(sent);
            // The target may end early, for example when the Process exits.
            let early = tokio::select! {
                biased;
                frame = next(reader) => Some(frame?),
                sent = &mut sent => {
                    sent?;
                    None
                }
            };
            match early {
                Some(frame) => frame,
                None => next(reader).await?,
            }
        }
    };
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(&frame));
    }
    match reply(&frame)? {
        ProcessReply::Written(bytes, closed) => Ok((bytes, closed)),
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}

async fn send_all<W, I>(writer: &mut FrameWriter<W>, mut input: I) -> Result<(), ClientError>
where
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
{
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = input.read(&mut buffer).await?;
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            break;
        };
        writer
            .data(Channel::Input, chunk)
            .await
            .map_err(ClientError::Stream)?;
    }
    writer
        .end(Channel::Input)
        .await
        .map_err(ClientError::Stream)?;
    writer.flush().await.map_err(ClientError::Stream)
}
