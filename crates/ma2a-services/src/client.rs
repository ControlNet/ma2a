//! Caller-side drivers of the built-in services, run after the caller wrote its `open`.
//!
//! Each driver follows its family's frame contract exactly and never half-closes its write
//! side before the target's terminal frame, because a caller EOF cancels the operation on
//! the local stream plane.

use std::{fmt, io};

use ma2a_core::service::{FrameKind, ServiceError, outcome_status};
use tokio::io::AsyncRead;

use crate::{Frame, FrameReader};

pub mod files;
pub mod process;
pub mod proxy;
pub mod resume;
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
        _ => Err(unexpected(&frame)),
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

/// Returns an error for a failed `outcome`, and nothing for a successful one.
pub(crate) fn failed(frame: &Frame) -> Result<(), ClientError> {
    match outcome_status(frame.payload()) {
        Ok(None) => Ok(()),
        Ok(Some(error)) => Err(ClientError::Failed(error)),
        Err(error) => Err(ClientError::Stream(ServiceError::from_protocol(error))),
    }
}

/// Classifies a frame the family contract does not allow at this point.
pub(crate) fn unexpected(frame: &Frame) -> ClientError {
    match frame.kind() {
        FrameKind::Reject => frame
            .rejection()
            .map_or_else(ClientError::Stream, ClientError::Rejected),
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
