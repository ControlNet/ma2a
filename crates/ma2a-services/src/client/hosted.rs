//! Caller side of Process v1: durable Processes hosted by the target Endpoint.
//!
//! Operations without a stream body answer one `outcome` after `accept`. `logs` and `attach`
//! carry output on channels 1 (stdout) and 2 (stderr), and report output that retention
//! dropped before it was read as `skipped` controls. `stdin` and `attach` with stdin send
//! channel 0 toward the Process.

mod attach;
mod stdin;

use ma2a_core::service::{
    Channel, FrameKind, LogFrom, ProcessAccept, ProcessReply, ServiceError, decode_skipped,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};

pub use attach::{Attached, attach};
pub use stdin::stdin;

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{Frame, FrameReader};

/// Output that retention dropped before this reader got it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Gaps {
    /// How many times output was skipped.
    pub count: u64,
    /// The cursor reading resumed at after the latest skip.
    pub resumed_at: Option<u64>,
}

impl Gaps {
    fn record(&mut self, frame: &Frame) -> Result<u64, ClientError> {
        let cursor = decode_skipped(frame.payload())
            .map_err(|error| ClientError::Stream(ServiceError::from_protocol(error)))?;
        self.count = self.count.saturating_add(1);
        self.resumed_at = Some(cursor);
        Ok(cursor)
    }
}

/// The answer to one `logs` read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogsRead {
    /// Where reading started and the window it saw.
    pub accept: ProcessAccept,
    /// The cursor to continue from.
    pub next: u64,
    /// Whether the Process ended and everything retained was read.
    pub ended: bool,
    /// Output dropped by retention while reading.
    pub gaps: Gaps,
}

/// Runs one Process operation without a stream body: `start`, `list`, `inspect`, `signal`,
/// `terminate`, or `delete`.
///
/// Returns the accept, or `None` when the outcome was answered from caller-side replay.
///
/// # Errors
/// Returns the refusal, the failed outcome, or the stream or protocol failure.
pub async fn call<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<(Option<ProcessAccept>, ProcessReply), ClientError> {
    let accept = match first(reader).await? {
        First::Finished(frame) => return Ok((None, reply(&frame)?)),
        First::Accepted(frame) => accepted(&frame)?,
    };
    let frame = next(reader).await?;
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(&frame));
    }
    Ok((Some(accept), reply(&frame)?))
}

/// Reads one `logs` answer from `from` of at most `max` bytes, the request's `max_bytes`,
/// writing stdout and stderr to their sinks as they arrive.
///
/// Every byte's cursor is known: the read starts where `from` resolves in the window the
/// accept reports, each record moves the cursor by its length, and a skip moves it forward to
/// the cursor it names. The reply's `next` must be where those bytes ended.
///
/// # Errors
/// Returns the refusal, the failed outcome, a local write failure, or the stream failure. An
/// answer that starts anywhere else, or answers without reading, fails before any byte reaches
/// a sink; one longer than `max` fails before its excess does; one whose skip goes back or whose
/// `next` is not where its bytes ended is a stream failure.
pub async fn logs<R, O, E>(
    reader: &mut FrameReader<R>,
    (output, error): (&mut O, &mut E),
    (from, max): (LogFrom, u32),
) -> Result<LogsRead, ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut gaps = Gaps::default();
    let mut left = usize::try_from(max).unwrap_or(usize::MAX);
    // A read reserves nothing, so it is never answered from replay.
    let First::Accepted(frame) = first(reader).await? else {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    };
    let accept = accepted(&frame)?;
    let mut reached = reading(&accept, from)?;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Data => {
                let (_, bytes) = frame.data().map_err(ClientError::Stream)?;
                left = left
                    .checked_sub(bytes.len())
                    .ok_or(ClientError::Stream(ServiceError::InvalidInput))?;
                reached = advanced(reached, bytes.len())?;
                deliver(&frame, (&mut *output, Some(&mut *error))).await?;
            }
            FrameKind::Control => reached = skipped(&mut gaps, &frame, reached)?,
            FrameKind::Outcome => {
                output.flush().await?;
                error.flush().await?;
                return match reply(&frame)? {
                    ProcessReply::Logs(next, ended) if next == reached => Ok(LogsRead {
                        accept,
                        next,
                        ended,
                        gaps,
                    }),
                    _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
                };
            }
            _ => return Err(unexpected(&frame)),
        }
    }
}

/// Checks that an accept reads from where `from` resolves in the window it reports, exactly as
/// the target must resolve it, and returns that start.
fn reading(accept: &ProcessAccept, from: LogFrom) -> Result<u64, ClientError> {
    match *accept {
        ProcessAccept::Reading {
            start,
            window,
            truncated,
            ..
        } if window.resolve(from) == Some((start, truncated)) => Ok(start),
        _ => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}

/// The cursor after `length` more bytes from `reached`; none lies past the last cursor.
fn advanced(reached: u64, length: usize) -> Result<u64, ClientError> {
    u64::try_from(length)
        .ok()
        .and_then(|length| reached.checked_add(length))
        .ok_or(ClientError::Stream(ServiceError::InvalidInput))
}

/// Records one skip, which may only move the cursor forward, and returns where it moved it.
fn skipped(gaps: &mut Gaps, frame: &Frame, reached: u64) -> Result<u64, ClientError> {
    let cursor = gaps.record(frame)?;
    if cursor < reached {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    }
    Ok(cursor)
}

/// Writes one output record to its sink; stderr without a sink is dropped.
async fn deliver<O, E>(
    frame: &Frame,
    (output, error): (&mut O, Option<&mut E>),
) -> Result<(), ClientError>
where
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    match frame.data().map_err(ClientError::Stream)? {
        (Channel::Output, bytes) => output.write_all(bytes).await?,
        (Channel::Error, bytes) => {
            if let Some(error) = error {
                error.write_all(bytes).await?;
            }
        }
        (Channel::Input, _) => return Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
    Ok(())
}

fn accepted(frame: &Frame) -> Result<ProcessAccept, ClientError> {
    ProcessAccept::decode(frame.payload())
        .map_err(|error| ClientError::Stream(ServiceError::from_protocol(error)))
}

fn reply(frame: &Frame) -> Result<ProcessReply, ClientError> {
    decoded(ProcessReply::decode_outcome(frame.payload()))
}
