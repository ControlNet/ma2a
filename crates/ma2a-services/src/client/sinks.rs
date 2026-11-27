//! A target's output and errors on their way to the caller's local sinks.

use std::{future::Future, io, time::Duration};

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use super::{ClientError, unexpected};
use crate::Frame;

/// Where the target's output and errors go, and what arrived on the way.
pub(super) struct Sinks<'a, O, E> {
    output: O,
    error: Option<E>,
    /// Whether the target may send on the error channel at all.
    errors: bool,
    /// Whether the target ended its output and its error channel.
    ended: (bool, bool),
    /// Bytes received on the output and the error channel.
    pub(super) received: (u64, u64),
    /// The first local write that failed; nothing is written after it.
    pub(super) unwritten: Option<io::Error>,
    /// Set once cancellation interrupts local delivery so remaining frames are only validated.
    pub(super) discarding: bool,
    /// Told once a write failed, so the inbound side cancels.
    lost: &'a Notify,
    /// Cancelled when the caller cancels or a local read fails.
    stop: &'a CancellationToken,
}

impl<'a, O, E> Sinks<'a, O, E>
where
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    /// Sinks for `output` and `error`; `errors` tells whether the target may send on the error
    /// channel, which is dropped when it has no sink.
    pub(super) fn new(
        (output, error): (O, Option<E>),
        errors: bool,
        (lost, stop): (&'a Notify, &'a CancellationToken),
    ) -> Self {
        Self {
            output,
            error,
            errors,
            ended: (false, false),
            received: (0, 0),
            unwritten: None,
            discarding: false,
            lost,
            stop,
        }
    }

    /// Takes one `data` or `end` of the target.
    ///
    /// The target sends only on the output channel, and on the error channel when it may, and
    /// nothing on a channel it ended. A failed local write is kept, and since nothing can
    /// observe the operation any more, the inbound side is asked to cancel it. Cancellation
    /// interrupts any blocked write and discards further output while still checking frames and
    /// byte counts.
    ///
    /// # Errors
    /// Returns `invalid_input` for a frame that breaks those rules, or any other frame.
    pub(super) async fn take(&mut self, frame: &Frame) -> Result<(), ClientError> {
        let invalid = || ClientError::Stream(ServiceError::InvalidInput);
        let (channel, bytes) = match frame.kind() {
            FrameKind::Data => {
                let (channel, bytes) = frame.data().map_err(ClientError::Stream)?;
                (channel, Some(bytes))
            }
            FrameKind::End => (frame.end().map_err(ClientError::Stream)?, None),
            _ => return Err(unexpected(frame)),
        };
        let (ended, count) = match channel {
            Channel::Output => (&mut self.ended.0, &mut self.received.0),
            Channel::Error if self.errors => (&mut self.ended.1, &mut self.received.1),
            Channel::Error | Channel::Input => return Err(invalid()),
        };
        if *ended {
            return Err(invalid());
        }
        let Some(bytes) = bytes else {
            *ended = true;
            return Ok(());
        };
        *count = count.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if channel == Channel::Error && self.error.is_none() {
            return Ok(());
        }
        if self.unwritten.is_some() || self.discarding || self.stop.is_cancelled() {
            self.discarding = true;
            return Ok(());
        }
        let written = match (channel, self.error.as_mut()) {
            (Channel::Error, Some(error)) => {
                write_until(write_flushed(error, bytes), self.stop).await
            }
            (Channel::Error, None) => Some(Ok(())),
            _ => write_until(write_flushed(&mut self.output, bytes), self.stop).await,
        };
        match written {
            Some(Ok(())) => {}
            Some(Err(failure)) => {
                self.unwritten = Some(failure);
                self.lost.notify_one();
            }
            None => self.discarding = true,
        }
        Ok(())
    }
}

/// Runs `write` until it finishes or `stop` is cancelled; preserves a completed write over a
/// simultaneous cancellation.
pub(super) async fn write_until<F: Future>(
    write: F,
    stop: &CancellationToken,
) -> Option<F::Output> {
    tokio::select! {
        biased;
        result = write => Some(result),
        () = stop.cancelled() => None,
    }
}

/// The answer of an operation from what the target answered with the first local write that
/// failed, and the local read that failed: a stream that broke the contract stays broken, and
/// any outcome gives way to the local failure, since the target only saw a `cancel`.
pub(super) fn settled<T>(
    (outcome, unwritten): (Result<T, ClientError>, Option<io::Error>),
    unread: Option<io::Error>,
) -> Result<T, ClientError> {
    match outcome {
        Err(ClientError::Stream(error)) => Err(ClientError::Stream(error)),
        outcome => unwritten
            .or(unread)
            .map_or(outcome, |failure| Err(ClientError::Local(failure))),
    }
}

async fn write_flushed<T: AsyncWrite + Unpin>(sink: &mut T, bytes: &[u8]) -> io::Result<()> {
    sink.write_all(bytes).await?;
    sink.flush().await
}

/// Writes `bytes` and flushes `local_write`, allowing up to `idle` without progress on each
/// step so a slow consumer still draining receives every byte while a stalled one times out.
pub(super) async fn write_bounded<LW: AsyncWrite + Unpin>(
    local_write: &mut LW,
    bytes: &[u8],
    idle: Duration,
) -> io::Result<()> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = tokio::time::timeout(idle, local_write.write(rest))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local write timed out"))??;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "local write returned zero bytes",
            ));
        }
        rest = rest.get(written..).unwrap_or_default();
    }
    tokio::time::timeout(idle, local_write.flush())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local flush timed out"))?
}

/// Shuts down `local_write`, allowing up to `idle` without progress before timing out.
pub(super) async fn shutdown_bounded<LW: AsyncWrite + Unpin>(
    local_write: &mut LW,
    idle: Duration,
) -> io::Result<()> {
    tokio::time::timeout(idle, local_write.shutdown())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local shutdown timed out"))?
}

/// Reads local input; `None` is EOF.
pub(super) async fn read_input<I: AsyncRead + Unpin>(
    input: Option<&mut I>,
    buffer: &mut [u8],
) -> io::Result<Option<usize>> {
    let Some(input) = input else {
        return Ok(None);
    };
    Ok(Some(input.read(buffer).await?).filter(|read| *read > 0))
}
