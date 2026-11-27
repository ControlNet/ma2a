//! Caller side of Exec v1 and Shell v1: local pipes or a terminal bound to a remote process.

use std::future::Future;

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, ProcessResult, ServiceError, TerminalSize,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Notify, mpsc},
};
use tokio_util::sync::CancellationToken;

use super::{
    ClientError, First, Sender, Written, decoded, drained, empty_accept, first_or_cancel, next,
    send_cancel,
    sinks::{Sinks, read_input, settled},
    write_watching,
};
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
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Streamed {
    /// The terminal summary.
    pub result: ProcessResult,
    /// Whether the outcome arrived without the run: a replay of an earlier request answers
    /// only its terminal, so none of the process's output was received.
    pub replayed: bool,
    /// Whether local delivery to the caller's sinks was interrupted or discarded.
    pub delivery_interrupted: bool,
}

/// Relays one admitted remote process until its terminal outcome.
///
/// Input and output run independently, so a process that echoes a large input can never
/// deadlock against a caller that is still sending it. Only EOF ends the process's input; a
/// failed local read cancels the process instead.
///
/// # Errors
/// Returns the refusal (`spawn_failed` when the program cannot start), the failed outcome,
/// the stream failure, a local I/O or interrupted-delivery failure on a `Completed` outcome,
/// or, once the process was cancelled, the failed local read.
pub async fn run<R, W, I, O, E, C>(
    streams: (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
    control: ProcessControl<C>,
) -> Result<ProcessResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let streamed = run_streamed(streams, local, control).await?;
    if streamed.delivery_interrupted && streamed.result.reason == EndReason::Completed {
        return Err(ClientError::Local(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "local output delivery was interrupted",
        )));
    }
    Ok(streamed.result)
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
    W: AsyncWrite + Unpin + Send,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let ProcessControl { resizes, cancel } = control;
    tokio::pin!(cancel);
    let (answer, sender) = first_or_cancel((reader, writer), cancel.as_mut()).await?;
    // A running process is always accepted first; an outcome without that is a replay.
    match answer {
        First::Finished(frame) => {
            let result = decoded(ProcessResult::decode_outcome(frame.payload()))?;
            return Ok(Streamed {
                result,
                replayed: true,
                delivery_interrupted: false,
            });
        }
        First::Accepted(frame) => empty_accept(&frame)?,
    }
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let (lost, stop) = (Notify::new(), CancellationToken::new());
    let errors = error.is_some();
    let mut outbound = Box::pin(outbound(
        reader,
        Sinks::new((output, error), errors, (&lost, &stop)),
    ));
    // A failed local read is kept even when the outcome arrives before the cancel is sent.
    let mut unread = None;
    // Once cancelled, nothing but the rest of that `cancel` is sent; the outcome still follows.
    let (answer, unwritten, delivery_interrupted) = match sender {
        Sender::Cancelled(rest) => {
            stop.cancel();
            drained(outbound.as_mut(), rest).await.unwrap_or((
                Err(ClientError::Stream(ServiceError::OutcomeUnknown)),
                None,
                false,
            ))
        }
        Sender::Open(writer) => {
            let control = ProcessControl { resizes, cancel };
            let mut inbound = Box::pin(inbound(
                writer,
                (input, control),
                (&lost, &stop, &mut unread),
            ));
            tokio::select! {
                answer = &mut outbound => answer,
                () = &mut inbound => drained(outbound.as_mut(), None)
                    .await
                    .unwrap_or((Err(ClientError::Stream(ServiceError::OutcomeUnknown)), None, false)),
            }
        }
    };
    Ok(Streamed {
        result: settled((answer, unwritten), unread)?,
        replayed: false,
        delivery_interrupted,
    })
}

enum InboundStop {
    None,
    SendCancel,
    SkipCancel,
}

/// Caller → target: input, resizes, and at most one `cancel`, until nothing is left to send;
/// a failed local read cancels instead of ending input, and is kept in `unread` first.
pub(super) async fn inbound<W, I, C>(
    writer: &mut FrameWriter<W>,
    (mut input, control): (Option<I>, ProcessControl<C>),
    (lost, stop_signal, unread): (&Notify, &CancellationToken, &mut Option<std::io::Error>),
) where
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
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    while input.is_some() || resizes.is_some() || !cancelled {
        let reading = input.is_some();
        let resizing = resizes.is_some();
        let stop = tokio::select! {
            biased;
            () = lost.notified(), if !cancelled => InboundStop::SendCancel,
            () = &mut cancel, if !cancelled => InboundStop::SendCancel,
            read = read_input(input.as_mut(), &mut buffer), if reading => match read {
                Ok(read) => {
                    let write = async {
                        match read.and_then(|length| buffer.get(..length)) {
                            Some(chunk) => writer.data(Channel::Input, chunk).await?,
                            None => writer.end(Channel::Input).await?,
                        }
                        // Flushed after the `end` too, which a buffered stream would keep.
                        writer.flush().await
                    };
                    match write_watching(write, (lost, cancel.as_mut(), stop_signal)).await {
                        Written::Done(res) => {
                            if read.is_none() || res.is_err() {
                                input = None;
                            }
                            InboundStop::None
                        }
                        Written::Cancelled { finished: true } => InboundStop::SendCancel,
                        Written::Cancelled { finished: false } => InboundStop::SkipCancel,
                    }
                }
                Err(error) => {
                    *unread = Some(error);
                    InboundStop::SendCancel
                }
            },
            size = recv(resizes.as_mut()), if resizing => {
                match size.map(TerminalSize::encode_resize) {
                    Some(Ok(resize)) => {
                        let write = async {
                            writer.frame(FrameKind::Control, &resize).await?;
                            writer.flush().await
                        };
                        match write_watching(write, (lost, cancel.as_mut(), stop_signal)).await {
                            Written::Done(_) => InboundStop::None,
                            Written::Cancelled { finished: true } => InboundStop::SendCancel,
                            Written::Cancelled { finished: false } => InboundStop::SkipCancel,
                        }
                    }
                    Some(Err(_)) => InboundStop::None,
                    None => {
                        resizes = None;
                        InboundStop::None
                    }
                }
            }
        };
        if !matches!(stop, InboundStop::None) {
            stop_signal.cancel();
            cancelled = true;
            input = None;
            resizes = None;
            if matches!(stop, InboundStop::SendCancel) {
                send_cancel(writer).await;
            }
        }
    }
}

/// Target → caller until the outcome; lost local output asks the inbound side to cancel.
/// Returns the answer and the first local write that failed.
///
/// The outcome counts exactly the bytes the target sent on each channel: an answer with other
/// counts is not the run that streamed.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    mut sinks: Sinks<'_, O, E>,
) -> (
    Result<ProcessResult, ClientError>,
    Option<std::io::Error>,
    bool,
)
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let answer = async {
        loop {
            let frame = next(reader).await?;
            if frame.kind() != FrameKind::Outcome {
                sinks.take(&frame).await?;
                continue;
            }
            let result = decoded(ProcessResult::decode_outcome(frame.payload()))?;
            if (result.output_bytes, result.error_bytes) != sinks.received {
                return Err(ClientError::Stream(ServiceError::IntegrityMismatch));
            }
            return Ok(result);
        }
    }
    .await;
    (answer, sinks.unwritten, sinks.discarding)
}

async fn recv(resizes: Option<&mut mpsc::Receiver<TerminalSize>>) -> Option<TerminalSize> {
    resizes?.recv().await
}
