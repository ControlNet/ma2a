//! Caller side of Exec v1 and Shell v1: local pipes or a terminal bound to a remote process.

use std::future::Future;

use ma2a_core::service::{
    Channel, FrameKind, MAX_DATA_CHUNK_LEN, ProcessResult, ServiceError, TerminalSize,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::{Notify, mpsc},
};

use super::{ClientError, First, decoded, empty_accept, first_or_cancel, next, unexpected};
use crate::{FrameReader, FrameWriter};

/// Local ends of one remote process.
#[expect(
    clippy::exhaustive_structs,
    reason = "a plain bundle of caller-owned ends, built by struct literal"
)]
#[derive(Debug)]
pub struct ProcessIo<I, O, E> {
    /// Forwarded as the process's input until EOF; `None` forwards nothing.
    pub input: Option<I>,
    /// Receives stdout (Exec) or terminal output (Shell).
    pub output: O,
    /// Receives stderr; Shell has none, and a target sending one breaks the contract.
    pub error: Option<E>,
}

/// Caller-side controls of one remote process.
#[expect(
    clippy::exhaustive_structs,
    reason = "a plain bundle of caller-owned controls, built by struct literal"
)]
#[derive(Debug)]
pub struct ProcessControl<C> {
    /// Terminal sizes to forward as `resize` controls (Shell only).
    pub resizes: Option<mpsc::Receiver<TerminalSize>>,
    /// Completes when the caller wants the process cancelled; the terminal still follows.
    pub cancel: C,
}

/// How one remote process ended, and whether its output reached the caller.
#[expect(
    clippy::exhaustive_structs,
    reason = "a plain pair a caller destructures"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streamed {
    /// The terminal summary.
    pub result: ProcessResult,
    /// Whether the outcome arrived without the run: a replay of an earlier request answers
    /// only its terminal, so none of the process's output was received.
    pub replayed: bool,
}

/// Relays one admitted remote process until its terminal outcome.
///
/// Input and output run independently, so a process that echoes a large input can never
/// deadlock against a caller that is still sending it. Only EOF ends the process's input; a
/// failed local read cancels the process instead.
///
/// # Errors
/// Returns the refusal (`spawn_failed` when the program cannot start), the failed outcome,
/// the stream failure, or, once the process was cancelled, the failed local read.
pub async fn run<R, W, I, O, E, C>(
    streams: (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
    control: ProcessControl<C>,
) -> Result<ProcessResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    Ok(run_streamed(streams, local, control).await?.result)
}

/// Like [`run`], and also tells whether the outcome was only replayed, without its output.
///
/// # Errors
/// As [`run`].
pub async fn run_streamed<R, W, I, O, E, C>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
    control: ProcessControl<C>,
) -> Result<Streamed, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let ProcessControl { resizes, cancel } = control;
    tokio::pin!(cancel);
    let (answer, cancelled) = first_or_cancel((reader, writer), cancel.as_mut()).await?;
    // A running process is always accepted first; an outcome without that is a replay.
    match answer {
        First::Finished(frame) => {
            let result = decoded(ProcessResult::decode_outcome(frame.payload()))?;
            return Ok(Streamed {
                result,
                replayed: true,
            });
        }
        First::Accepted(frame) => empty_accept(&frame)?,
    }
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let lost = Notify::new();
    let outbound = outbound(reader, (output, error), &lost);
    // Once cancelled, nothing more is sent; the outcome still follows.
    let result = if cancelled {
        outbound.await
    } else {
        let inbound = inbound(writer, (input, ProcessControl { resizes, cancel }), &lost);
        tokio::pin!(outbound, inbound);
        tokio::select! {
            biased;
            result = &mut outbound => result,
            failed = &mut inbound => finished(outbound.await, failed),
        }
    }?;
    Ok(Streamed {
        result,
        replayed: false,
    })
}

/// The outcome of a remote process whose input side ended with `failed`: a failed local read
/// is the answer, since the target only saw a `cancel`.
pub(super) fn finished<T>(
    outcome: Result<T, ClientError>,
    failed: Option<std::io::Error>,
) -> Result<T, ClientError> {
    failed.map_or(outcome, |failure| Err(ClientError::Local(failure)))
}

/// Caller → target: input, resizes, and at most one `cancel`, until nothing is left to send;
/// returns a failed local read, which cancels instead of ending input.
pub(super) async fn inbound<W, I, C>(
    writer: &mut FrameWriter<W>,
    (mut input, control): (Option<I>, ProcessControl<C>),
    lost: &Notify,
) -> Option<std::io::Error>
where
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    C: Future<Output = ()>,
{
    let ProcessControl {
        mut resizes,
        cancel,
    } = control;
    tokio::pin!(cancel);
    let mut cancelled = false;
    let mut failed = None;
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    while input.is_some() || resizes.is_some() || !cancelled {
        let reading = input.is_some();
        let resizing = resizes.is_some();
        let stop = tokio::select! {
            biased;
            () = lost.notified(), if !cancelled => true,
            () = &mut cancel, if !cancelled => true,
            read = read_input(input.as_mut(), &mut buffer), if reading => match read {
                Ok(read) => {
                    let forwarded = match read.and_then(|length| buffer.get(..length)) {
                        Some(chunk) => writer.data(Channel::Input, chunk).await,
                        None => writer.end(Channel::Input).await,
                    };
                    // Flushed after the `end` too, which a buffered stream would keep.
                    let flushed = forwarded.and(writer.flush().await);
                    if read.is_none() || flushed.is_err() {
                        input = None;
                    }
                    false
                }
                Err(error) => {
                    failed = Some(error);
                    true
                }
            },
            size = recv(resizes.as_mut()), if resizing => {
                match size.map(TerminalSize::encode_resize) {
                    Some(Ok(resize)) => send(writer, FrameKind::Control, &resize).await,
                    Some(Err(_)) => {}
                    None => resizes = None,
                }
                false
            }
        };
        if stop {
            cancelled = true;
            input = None;
            resizes = None;
            send(writer, FrameKind::Cancel, &[]).await;
        }
    }
    failed
}

/// Target → caller until the outcome; lost local output asks the inbound side to cancel.
///
/// The target sends only on the output channel, and on the error channel when there is one,
/// and its outcome counts exactly the bytes it sent on each: an answer with other counts is
/// not the run that streamed.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    (mut output, mut error): (O, Option<E>),
    lost: &Notify,
) -> Result<ProcessResult, ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let invalid = || ClientError::Stream(ServiceError::InvalidInput);
    let sends_on = |channel, error: bool| {
        matches!(
            (channel, error),
            (Channel::Output, _) | (Channel::Error, true)
        )
    };
    let mut delivering = true;
    // Bytes received on the output and the error channel.
    let mut received = (0_u64, 0_u64);
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Outcome => {
                let result = decoded(ProcessResult::decode_outcome(frame.payload()))?;
                if (result.output_bytes, result.error_bytes) != received {
                    return Err(ClientError::Stream(ServiceError::IntegrityMismatch));
                }
                return Ok(result);
            }
            FrameKind::Data => {
                let (channel, bytes) = frame.data().map_err(ClientError::Stream)?;
                if !sends_on(channel, error.is_some()) {
                    return Err(invalid());
                }
                let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                let count = match channel {
                    Channel::Output => &mut received.0,
                    Channel::Error | Channel::Input => &mut received.1,
                };
                *count = count.saturating_add(length);
                let written = match (channel, error.as_mut()) {
                    _ if !delivering => Ok(()),
                    (Channel::Error, Some(error)) => write_flushed(error, bytes).await,
                    _ => write_flushed(&mut output, bytes).await,
                };
                if written.is_err() {
                    // Nothing can observe the process any more, so it is cancelled.
                    delivering = false;
                    lost.notify_one();
                }
            }
            FrameKind::End => {
                if !sends_on(frame.end().map_err(ClientError::Stream)?, error.is_some()) {
                    return Err(invalid());
                }
            }
            _ => return Err(unexpected(&frame)),
        }
    }
}

pub(super) async fn write_flushed<T: AsyncWrite + Unpin>(
    sink: &mut T,
    bytes: &[u8],
) -> std::io::Result<()> {
    sink.write_all(bytes).await?;
    sink.flush().await
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

async fn recv(resizes: Option<&mut mpsc::Receiver<TerminalSize>>) -> Option<TerminalSize> {
    resizes?.recv().await
}

/// Sends one control frame; a failed write is answered by the target's terminal anyway.
async fn send<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>, kind: FrameKind, payload: &[u8]) {
    let _sent = async {
        writer.frame(kind, payload).await?;
        writer.flush().await
    }
    .await;
}
