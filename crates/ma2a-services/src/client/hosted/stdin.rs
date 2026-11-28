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

/// How far input went: bytes completed, bytes started (including any in-flight write), and
/// whether `end` has started (so all `data` frames completed).
#[derive(Clone, Copy, Default)]
struct Progress {
    sent: u64,
    started: u64,
    ended: bool,
}

/// Writes local input to the Process stdin until EOF, then reports what was delivered.
///
/// `close` is what the `stdin` request asked for. Returns the bytes the Process received and
/// whether its stdin is now closed. The target may answer before all input was sent, for example
/// once the Process exited, which closes its stdin; a target that stopped reading still reports
/// why. An answer counting more bytes than were sent is a stream failure, and so is one that
/// leaves stdin open before `end` was sent, after `close` was asked for, or without every byte.
///
/// # Errors
/// Returns the refusal (`writer_busy`, `stdin_closed`, `not_running`), the failed outcome,
/// the stream failure, or the failed local read (after cancelling the write).
pub async fn stdin<R, W, I>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (input, close): (I, bool),
) -> Result<(u64, bool), ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
{
    let frame = match first(reader).await? {
        // A replayed write was counted when it ran.
        First::Finished(frame) => return written(&frame, (None, close)),
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
        // A target that stopped reading may still have said why; if the stream broke without
        // a terminal frame, `next` already reports `outcome_unknown` (or `invalid_input` for
        // a malformed frame).
        Err(Ok(()) | Err(ClientError::Stream(_))) => next(reader).await?,
        // Local input failed: cancel the write within the grace so the target releases its
        // writer lease even if the caller keeps this stream open.
        Err(Err(error)) => {
            crate::client::cancel_or_terminal(reader, writer).await;
            return Err(error);
        }
    };
    written(&frame, (Some(progress), close))
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
        let len = u64::try_from(read).unwrap_or(u64::MAX);
        progress.started = progress.started.saturating_add(len);
        writer
            .data(Channel::Input, chunk)
            .await
            .map_err(ClientError::Stream)?;
        progress.sent = progress.started;
    }
    // The target may answer `end` before its write or flush returns.
    progress.ended = true;
    writer
        .end(Channel::Input)
        .await
        .map_err(ClientError::Stream)?;
    writer.flush().await.map_err(ClientError::Stream)
}

/// Checks that an outcome leaves stdin open only without `close`, and, when `progress` is
/// known, counts no more than was sent and leaves stdin open only after `end` and with every
/// byte taken, since a Process that stopped taking input closed its stdin. A replayed one has
/// no `progress`; the `close` it was asked for still holds.
fn written(
    frame: &Frame,
    (progress, close): (Option<Progress>, bool),
) -> Result<(u64, bool), ClientError> {
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(frame));
    }
    let answers = |(bytes, closed): (u64, bool), progress: Progress| {
        if closed {
            bytes <= progress.started
        } else {
            progress.ended && bytes == progress.sent
        }
    };
    match reply(frame)? {
        ProcessReply::Written(bytes, closed)
            if (closed || !close)
                && progress.is_none_or(|progress| answers((bytes, closed), progress)) =>
        {
            Ok((bytes, closed))
        }
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}
