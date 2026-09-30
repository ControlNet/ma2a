//! Terminal frames shared by every host and client-cancellation watching.

use std::time::Duration;

use ma2a_core::{
    ProtocolError,
    service::{FrameKind, ServiceError},
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    FrameReader, FrameWriter,
    report::{OperationReport, Termination},
};

/// Longest wait for progress on a streaming transfer before it fails as timed out.
pub(crate) const STALL_DEADLINE: Duration = Duration::from_secs(30);

/// Sends `reject`, half-closes, and reports that nothing happened.
pub(crate) async fn reject<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    error: ServiceError,
) -> OperationReport {
    let _sent = writer.reject(error).await;
    let _closed = writer.shutdown().await;
    OperationReport::new(Termination::Rejected(error))
}

/// Sends one encoded terminal `outcome`, half-closes, and reports its class.
pub(crate) async fn outcome<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    (encoded, failure): (Result<Vec<u8>, ProtocolError>, Option<ServiceError>),
    (bytes_in, bytes_out): (u64, u64),
) -> OperationReport {
    let sent = match encoded {
        Ok(encoded) => writer.frame(FrameKind::Outcome, &encoded).await,
        Err(_) => Err(ServiceError::Internal),
    };
    let _closed = writer.shutdown().await;
    let termination = match (sent, failure) {
        (Err(error), _) | (Ok(()), Some(error)) => Termination::Failed(error),
        (Ok(()), None) => Termination::Completed,
    };
    OperationReport::new(termination).with_bytes(bytes_in, bytes_out)
}

/// Reports an operation that started but whose stream failed before a terminal frame.
pub(crate) const fn lost(
    error: ServiceError,
    (bytes_in, bytes_out): (u64, u64),
) -> OperationReport {
    OperationReport::new(Termination::Failed(error)).with_bytes(bytes_in, bytes_out)
}

/// Resolves once the client cancels, resets, or violates the protocol.
///
/// A clean client half-close is not a cancellation: the future then never resolves, so the
/// operation continues until its own terminal.
pub(crate) async fn client_cancelled<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> ServiceError {
    match reader.next().await {
        Ok(None) => std::future::pending().await,
        Ok(Some(frame)) if frame.kind() == FrameKind::Cancel => ServiceError::Cancelled,
        Ok(Some(_)) => ServiceError::InvalidInput,
        Err(error) => error,
    }
}
