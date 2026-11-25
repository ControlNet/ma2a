//! Stdin writes: channel 0 toward the Process until the caller's EOF.

use ma2a_core::service::{
    Channel, FrameKind, MAX_DATA_CHUNK_LEN, ProcessAccept, ProcessReply, ServiceError,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};

use super::{accepted, reply};
use crate::{
    Frame, FrameReader, FrameWriter,
    client::{ClientError, First, first, next, unexpected},
};

/// How far input went: the bytes sent, and whether `end` followed them.
type Progress = (u64, bool);

/// Writes local input to the Process stdin until EOF, then reports what was delivered.
///
/// Returns the bytes the Process received and whether its stdin is now closed. The target may
/// answer before all input was sent, for example once the Process exited; a target that stopped
/// reading still reports why. An answer counting more bytes than were sent, or an open stdin
/// before `end` was sent, is a stream failure.
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
        // A replayed write was counted when it ran.
        First::Finished(frame) => return written(&frame, None),
        First::Accepted(frame) => frame,
    };
    if accepted(&frame)? != ProcessAccept::Admitted {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    }
    let mut progress = Progress::default();
    let early = {
        let sent = send_all(writer, input, &mut progress);
        tokio::pin!(sent);
        // The target may end early, for example when the Process exits.
        tokio::select! {
            biased;
            frame = next(reader) => Ok(frame?),
            sent = &mut sent => Err(sent),
        }
    };
    let frame = match early {
        Ok(frame) => frame,
        Err(Ok(())) => next(reader).await?,
        // A target that stopped reading may still have said why.
        Err(Err(error @ ClientError::Stream(_))) => next(reader).await.map_err(|_| error)?,
        // Local input failed: the target still waits for more of it.
        Err(Err(error)) => return Err(error),
    };
    written(&frame, Some(progress))
}

async fn send_all<W, I>(
    writer: &mut FrameWriter<W>,
    mut input: I,
    progress: &mut Progress,
) -> Result<(), ClientError>
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
        progress.0 += u64::try_from(read).unwrap_or(u64::MAX);
    }
    writer
        .end(Channel::Input)
        .await
        .map_err(ClientError::Stream)?;
    writer.flush().await.map_err(ClientError::Stream)?;
    progress.1 = true;
    Ok(())
}

/// Checks that an outcome counts no more than was sent, and leaves stdin open only after `end`;
/// a replayed one, without `progress`, was checked when it ran.
fn written(frame: &Frame, progress: Option<(u64, bool)>) -> Result<(u64, bool), ClientError> {
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(frame));
    }
    match reply(frame)? {
        ProcessReply::Written(bytes, closed)
            if progress.is_none_or(|(sent, ended)| bytes <= sent && (closed || ended)) =>
        {
            Ok((bytes, closed))
        }
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}
