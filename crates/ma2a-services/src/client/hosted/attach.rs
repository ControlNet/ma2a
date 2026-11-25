//! Attachments: output back from the Process, and with stdin, channel 0 toward it.
//!
//! Detaching never stops the Process. The caller sends `cancel` and still reads the
//! target's `outcome`, which reports where the reader was.

use std::future::Future;

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, ProcessAccept, ProcessReply, ProcessSummary,
    ServiceError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

use super::{Gaps, accepted, deliver, reply};
use crate::{
    FrameReader, FrameWriter,
    client::{ClientError, First, first_or_cancel, next, process::ProcessIo, unexpected},
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

/// Follows one Process until it ends or `detach` completes.
///
/// With `input`, local input is forwarded as stdin and its EOF closes the Process stdin;
/// the `open` must then have asked for the stdin writer. A failed local output write
/// detaches, because nothing can observe the Process any more.
///
/// # Errors
/// Returns the refusal (`writer_busy` when another attachment holds stdin), the failed
/// outcome, or the stream failure.
pub async fn attach<R, W, I, O, E, D>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
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
        First::Finished(frame) => return attached((None, &frame), Gaps::default()),
        First::Accepted(frame) => accepted(&frame)?,
    };
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let lost = Notify::new();
    let outbound = outbound(reader, (output, error), &lost);
    // Once detached, nothing more is sent; the outcome still follows.
    let (frame, gaps) = if detached {
        outbound.await
    } else {
        let inbound = inbound(writer, (input, detach), &lost);
        tokio::pin!(outbound, inbound);
        tokio::select! {
            biased;
            read = &mut outbound => read,
            () = &mut inbound => outbound.await,
        }
    }?;
    attached((Some(accept), &frame), gaps)
}

/// Caller → target: stdin until EOF, then at most one `cancel` on detach or lost output.
async fn inbound<W, I, D>(
    writer: &mut FrameWriter<W>,
    (mut input, detach): (Option<I>, D),
    lost: &Notify,
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
                let forwarded = match read.and_then(|length| buffer.get(..length)) {
                    Some(chunk) => writer.data(Channel::Input, chunk).await,
                    None => writer.end(Channel::Input).await,
                };
                if read.is_none() || forwarded.and(writer.flush().await).is_err() {
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

/// Target → caller until the outcome; a failed local write asks the inbound side to detach.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    (mut output, mut error): (O, Option<E>),
    lost: &Notify,
) -> Result<(crate::Frame, Gaps), ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut gaps = Gaps::default();
    let mut delivering = true;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Outcome => return Ok((frame, gaps)),
            FrameKind::Control => {
                gaps.record(&frame)?;
            }
            FrameKind::Data if delivering => {
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
                    Err(ClientError::Local(_)) => {
                        delivering = false;
                        lost.notify_one();
                    }
                    Err(failure) => return Err(failure),
                }
            }
            FrameKind::Data => {}
            _ => return Err(unexpected(&frame)),
        }
    }
}

/// Reads local input; `None` is EOF, and a read failure ends input the same way.
async fn read_input<I: AsyncRead + Unpin>(
    input: Option<&mut I>,
    buffer: &mut [u8],
) -> Option<usize> {
    match input?.read(buffer).await {
        Ok(0) | Err(_) => None,
        Ok(read) => Some(read),
    }
}

fn attached(
    (accept, frame): (Option<ProcessAccept>, &crate::Frame),
    gaps: Gaps,
) -> Result<Attached, ClientError> {
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(frame));
    }
    match reply(frame)? {
        ProcessReply::Detached(reason, summary, cursor) => Ok(Attached {
            accept,
            reason,
            summary,
            cursor,
            gaps,
        }),
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}
