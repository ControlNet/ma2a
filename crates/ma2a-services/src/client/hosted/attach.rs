//! Attachments: output back from the Process, and with stdin, channel 0 toward it.
//!
//! Detaching never stops the Process. The caller sends `cancel` and still reads the
//! target's `outcome`, which reports where the reader was.

use std::future::Future;

use ma2a_core::service::{
    Channel, EndReason, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessAccept, ProcessId,
    ProcessReply, ProcessSummary, ServiceError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

use super::{Gaps, accepted, advanced, deliver, reading, reply, skipped, until};
use crate::{
    FrameReader, FrameWriter,
    client::{
        ClientError, First, first_or_cancel, next, process::ProcessIo, sinks::settled, unexpected,
    },
};

/// How one attachment ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Attached {
    /// Where reading started and the window it saw; `None` when answered from replay.
    pub accept: Option<ProcessAccept>,
    /// `completed` once the Process ended, `cancelled` when the caller detached.
    pub reason: EndReason,
    /// The Process as it now is.
    pub summary: ProcessSummary,
    /// The cursor to continue reading from.
    pub cursor: u64,
    /// Output dropped by retention while attached.
    pub gaps: Gaps,
}

/// Follows Process `id` from `from` until it ends or `detach` completes.
///
/// With `input`, local input is forwarded as stdin and its EOF closes the Process stdin;
/// the `open` must then have asked for the stdin writer. A failed local read detaches
/// instead, leaving the Process stdin open for a later writer. A failed local output write
/// detaches too, because nothing can observe the Process any more.
///
/// Every byte's cursor is known, as for `logs`: the attachment starts where `from` resolves in
/// the window the accept reports, and the outcome must name this Process and the cursor its
/// bytes and skips reached. A completed one must report the Process ended, at or past the end
/// of that window. Only an attachment with stdin, a mutation, may be answered from replay.
///
/// # Errors
/// Returns the refusal (`writer_busy` when another attachment holds stdin), the stream
/// failure, the failed local write or read that detached, or the failed outcome.
pub async fn attach<R, W, I, O, E, D>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    ((id, from), local): ((ProcessId, LogFrom), ProcessIo<I, O, E>),
    detach: D,
) -> Result<Attached, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    D: Future<Output = ()>,
{
    tokio::pin!(detach);
    let (answer, detached) = first_or_cancel((reader, writer), detach.as_mut()).await?;
    let accept = match answer {
        First::Finished(frame) if local.input.is_some() => {
            return attached((None, &frame), (id, None), Gaps::default());
        }
        First::Finished(_) => return Err(ClientError::Stream(ServiceError::InvalidInput)),
        First::Accepted(frame) => accepted(&frame)?,
    };
    let start = reading(&accept, from)?;
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let lost = Notify::new();
    let outbound = outbound(reader, (output, error), (&lost, start));
    // A failed local read is kept even when the outcome arrives before the detach is sent.
    let mut unread = None;
    // Once detached, nothing more is sent; the outcome still follows.
    let (frame, gaps, reached, unwritten) = if detached {
        outbound.await?
    } else {
        let inbound = inbound(writer, (input, detach), (&lost, &mut unread));
        tokio::pin!(outbound, inbound);
        tokio::select! {
            biased;
            read = &mut outbound => read?,
            () = &mut inbound => outbound.await?,
        }
    };
    let attached = attached((Some(accept), &frame), (id, Some(reached)), gaps);
    settled((attached, unwritten), unread)
}

/// Caller → target: stdin until EOF, then at most one `cancel` on detach, lost output, or a
/// failed local read, which it keeps in `unread`.
async fn inbound<W, I, D>(
    writer: &mut FrameWriter<W>,
    (mut input, detach): (Option<I>, D),
    (lost, unread): (&Notify, &mut Option<std::io::Error>),
) where
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    D: Future<Output = ()>,
{
    tokio::pin!(detach);
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let reading = input.is_some();
        tokio::select! {
            biased;
            () = lost.notified() => break,
            () = &mut detach => break,
            read = read_input(input.as_mut(), &mut buffer), if reading => {
                // Only EOF ends the Process stdin, which is for good; a failed read detaches.
                let read = match read {
                    Ok(read) => read,
                    Err(error) => {
                        *unread = Some(error);
                        break;
                    }
                };
                let forwarded = match read.and_then(|length| buffer.get(..length)) {
                    Some(chunk) => writer.data(Channel::Input, chunk).await,
                    None => writer.end(Channel::Input).await,
                };
                // Flushed after the `end` too, which a buffered stream would keep.
                let flushed = forwarded.and(writer.flush().await);
                if read.is_none() || flushed.is_err() {
                    input = None;
                }
            }
        }
    }
    // A failed write is answered by the target's terminal anyway.
    let _cancelled = async {
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await
    }
    .await;
}

/// What the target sent until its outcome: the outcome, the gaps, the cursor reached, and the
/// first local write that failed.
type Received = (crate::Frame, Gaps, u64, Option<std::io::Error>);

/// Target → caller until the outcome, from cursor `start`. A failed local write asks the
/// inbound side to detach.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    (mut output, mut error): (O, Option<E>),
    (lost, start): (&Notify, u64),
) -> Result<Received, ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut gaps = Gaps::default();
    let mut reached = start;
    let mut unwritten = None;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Outcome => return Ok((frame, gaps, reached, unwritten)),
            FrameKind::Control => reached = skipped(&mut gaps, &frame, reached)?,
            FrameKind::Data => {
                let bytes = match frame.data().map_err(ClientError::Stream)? {
                    (Channel::Input, _) => {
                        return Err(ClientError::Stream(ServiceError::InvalidInput));
                    }
                    (_, bytes) => bytes,
                };
                reached = advanced(reached, bytes.len())?;
                if unwritten.is_some() {
                    continue;
                }
                let delivered = async {
                    deliver(&frame, (&mut output, error.as_mut())).await?;
                    output.flush().await?;
                    if let Some(error) = error.as_mut() {
                        error.flush().await?;
                    }
                    Ok::<(), ClientError>(())
                };
                match delivered.await {
                    Ok(()) => {}
                    Err(ClientError::Local(failure)) => {
                        unwritten = Some(failure);
                        lost.notify_one();
                    }
                    Err(failure) => return Err(failure),
                }
            }
            _ => return Err(unexpected(&frame)),
        }
    }
}

/// Reads local input; `None` is EOF.
async fn read_input<I: AsyncRead + Unpin>(
    input: Option<&mut I>,
    buffer: &mut [u8],
) -> std::io::Result<Option<usize>> {
    let Some(input) = input else {
        return Ok(None);
    };
    Ok(Some(input.read(buffer).await?).filter(|read| *read > 0))
}

/// Checks that an outcome reports this Process detached at the cursor `reached`, when known.
/// An attachment completes only once the Process ended and every byte retained when it began
/// was sent.
fn attached(
    (accept, frame): (Option<ProcessAccept>, &crate::Frame),
    (id, reached): (ProcessId, Option<u64>),
    gaps: Gaps,
) -> Result<Attached, ClientError> {
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(frame));
    }
    let completes = |summary: &ProcessSummary, cursor: u64| {
        summary.state.is_terminal() && accept.is_none_or(|accept| cursor >= until(&accept))
    };
    match reply(frame)? {
        ProcessReply::Detached(reason, summary, cursor)
            if summary.id == id
                && reached.is_none_or(|reached| cursor == reached)
                && (reason != EndReason::Completed || completes(&summary, cursor)) =>
        {
            Ok(Attached {
                accept,
                reason,
                summary,
                cursor,
                gaps,
            })
        }
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}
