//! A target's output and errors on their way to the caller's local sinks.

use std::io;

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

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
    /// Told once a write failed, so the inbound side cancels.
    lost: &'a Notify,
}

impl<'a, O, E> Sinks<'a, O, E>
where
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    /// Sinks for `output` and `error`; `errors` tells whether the target may send on the error
    /// channel, which is dropped when it has no sink.
    pub(super) fn new((output, error): (O, Option<E>), errors: bool, lost: &'a Notify) -> Self {
        Self {
            output,
            error,
            errors,
            ended: (false, false),
            received: (0, 0),
            unwritten: None,
            lost,
        }
    }

    /// Takes one `data` or `end` of the target.
    ///
    /// The target sends only on the output channel, and on the error channel when it may, and
    /// nothing on a channel it ended. A failed local write is kept, and since nothing can
    /// observe the operation any more, the inbound side is asked to cancel it.
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
        if self.unwritten.is_some() {
            return Ok(());
        }
        let written = match (channel, self.error.as_mut()) {
            (Channel::Error, Some(error)) => write_flushed(error, bytes).await,
            (Channel::Error, None) => Ok(()),
            _ => write_flushed(&mut self.output, bytes).await,
        };
        if let Err(failure) = written {
            self.unwritten = Some(failure);
            self.lost.notify_one();
        }
        Ok(())
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
