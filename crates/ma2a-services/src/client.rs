//! Caller-side drivers of the built-in services, run after the caller wrote its `open`.
//!
//! Each driver follows its family's frame contract exactly and never half-closes its write
//! side before the target's terminal frame, because a caller EOF cancels the operation on
//! the local stream plane.

use std::{fmt, io, pin::Pin};

use ma2a_core::service::{EMPTY_ACCEPT, FrameKind, ServiceError, outcome_status};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{Frame, FrameReader, FrameWriter};

pub mod agent;
pub mod answer;
pub mod files;
pub mod hosted;
pub mod process;
pub mod proxy;
pub mod resume;
pub mod service;
mod sinks;
pub mod terminal;
pub mod transfer;

/// Why a caller-side operation did not succeed.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// The target refused before any effect.
    Rejected(ServiceError),
    /// The operation ran and failed, or its effect is unknown.
    Failed(ServiceError),
    /// The stream broke the family contract or ended without a terminal frame.
    Stream(ServiceError),
    /// A local file, pipe, or terminal operation failed.
    Local(io::Error),
}

impl ClientError {
    /// Returns the service status behind this error, if it has one.
    pub const fn status(&self) -> Option<ServiceError> {
        match self {
            Self::Rejected(error) | Self::Failed(error) | Self::Stream(error) => Some(*error),
            Self::Local(_) => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The stable status name comes first, so scripts can match it.
        match self {
            Self::Rejected(error) => {
                write!(
                    formatter,
                    "{}: target refused the operation: {error}",
                    error.name()
                )
            }
            Self::Failed(error) => write!(formatter, "{}: operation failed: {error}", error.name()),
            Self::Stream(error) => {
                write!(
                    formatter,
                    "{}: service stream failed: {error}",
                    error.name()
                )
            }
            Self::Local(error) => write!(formatter, "local I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Local(error) => Some(error),
            Self::Rejected(error) | Self::Failed(error) | Self::Stream(error) => Some(error),
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::Local(error)
    }
}

/// The target's first answer to an `open`.
#[derive(Debug)]
pub(crate) enum First {
    /// The operation was admitted; the frame carries the family's accept payload.
    Accepted(Frame),
    /// The operation already ended, or was answered from replay, with this successful outcome.
    Finished(Frame),
}

/// Reads the target's first answer, turning a refusal or failed outcome into an error.
pub(crate) async fn first<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<First, ClientError> {
    let frame = next(reader).await?;
    match frame.kind() {
        FrameKind::Accept => Ok(First::Accepted(frame)),
        FrameKind::Outcome => {
            failed(&frame)?;
            Ok(First::Finished(frame))
        }
        FrameKind::Reject => Err(refused(&frame)),
        _ => Err(unexpected(&frame)),
    }
}

/// The rest of sending a `cancel`: a failed write is answered by the target's terminal anyway.
pub(crate) type Cancelling<'w> = Pin<Box<dyn Future<Output = ()> + Send + 'w>>;

/// The caller's write side once the target's first answer arrived.
pub(crate) enum Sender<'w, W> {
    /// Not cancelled: the caller may still send.
    Open(&'w mut FrameWriter<W>),
    /// Cancelled, with the rest of that `cancel` when it is still on its way. That rest must go
    /// on while the caller reads the target's answer: the target may need it to end.
    Cancelled(Option<Cancelling<'w>>),
}

/// Reads the target's first answer while watching `cancel`, and hands back the write side:
/// still open, or cancelled.
///
/// A cancel before that answer is sent at once: a unary operation may answer only with its
/// outcome, so waiting for its `accept` would never let the target hear it. The answer is
/// still read while the `cancel` is on its way, since the target may answer before it reads.
pub(crate) async fn first_or_cancel<'w, R, W, C>(
    (reader, writer): (&mut FrameReader<R>, &'w mut FrameWriter<W>),
    cancel: Pin<&mut C>,
) -> Result<(First, Sender<'w, W>), ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send,
    C: Future<Output = ()>,
{
    let answer = first(reader);
    tokio::pin!(answer);
    tokio::select! {
        biased;
        first = &mut answer => return first.map(|first| (first, Sender::Open(writer))),
        () = cancel => {}
    }
    let mut sending: Cancelling<'w> = Box::pin(async move {
        let _sent = async {
            writer.frame(FrameKind::Cancel, &[]).await?;
            writer.flush().await
        }
        .await;
    });
    tokio::select! {
        biased;
        first = &mut answer => Ok((first?, Sender::Cancelled(Some(sending)))),
        () = &mut sending => Ok((answer.await?, Sender::Cancelled(None))),
    }
}

/// Reads the answer `outcome` gives while the rest of a `cancel`, if any, goes on; the answer
/// does not wait for it.
pub(crate) async fn drained<T>(
    outcome: impl Future<Output = T>,
    rest: Option<Cancelling<'_>>,
) -> T {
    let Some(rest) = rest else {
        return outcome.await;
    };
    tokio::pin!(outcome);
    tokio::select! {
        answer = &mut outcome => answer,
        () = rest => outcome.await,
    }
}

/// Reads one frame, treating a stream that ends without a terminal as an unknown outcome.
pub(crate) async fn next<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<Frame, ClientError> {
    reader
        .next()
        .await
        .map_err(ClientError::Stream)?
        .ok_or(ClientError::Stream(ServiceError::OutcomeUnknown))
}

/// Checks that an `accept` carries the empty header of a family that sends none.
pub(crate) fn empty_accept(frame: &Frame) -> Result<(), ClientError> {
    if frame.payload() == EMPTY_ACCEPT {
        Ok(())
    } else {
        Err(ClientError::Stream(ServiceError::InvalidInput))
    }
}

/// Returns an error for a failed `outcome`, and nothing for a successful one.
pub(crate) fn failed(frame: &Frame) -> Result<(), ClientError> {
    match outcome_status(frame.payload()) {
        Ok(None) => Ok(()),
        Ok(Some(error)) => Err(ClientError::Failed(error)),
        Err(error) => Err(ClientError::Stream(ServiceError::from_protocol(error))),
    }
}

/// The refusal a `reject` answering an `open` carries.
pub(crate) fn refused(frame: &Frame) -> ClientError {
    frame
        .rejection()
        .map_or_else(ClientError::Stream, ClientError::Rejected)
}

/// Classifies a frame the family contract does not allow at this point, after the target's
/// first answer: a `reject` then would claim that an admitted operation had no effect.
pub(crate) fn unexpected(frame: &Frame) -> ClientError {
    match frame.kind() {
        FrameKind::Outcome => failed(frame)
            .err()
            .unwrap_or(ClientError::Stream(ServiceError::InvalidInput)),
        _ => ClientError::Stream(ServiceError::InvalidInput),
    }
}

/// Maps a decoded family outcome to the caller's result.
pub(crate) fn decoded<T>(
    outcome: Result<Result<T, ServiceError>, ma2a_core::ProtocolError>,
) -> Result<T, ClientError> {
    match outcome {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(ClientError::Failed(error)),
        Err(error) => Err(ClientError::Stream(ServiceError::from_protocol(error))),
    }
}
