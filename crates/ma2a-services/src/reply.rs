//! Terminal frames shared by every host and client-cancellation watching.

use std::time::Duration;

use ma2a_core::{
    ProtocolError,
    service::{FrameKind, ServiceError},
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    FrameReader, FrameWriter,
    report::{OperationEnd, OperationReport},
};

/// Longest wait for progress on a streaming transfer before it fails as timed out.
pub(crate) const STALL_DEADLINE: Duration = Duration::from_secs(30);

/// Longest a host waits to write the frame that opens or ends an operation, and to close.
///
/// A caller that stops reading cannot hold the host, its permits, or its child past this.
pub const FRAME_GRACE: Duration = Duration::from_secs(5);

/// Writes `accept` within [`FRAME_GRACE`], unless `shutdown` fires first.
///
/// # Errors
/// Returns `unavailable` on shutdown, `timed_out` when the grace passes, or the write error.
pub(crate) async fn accept<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    payload: &[u8],
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<(), ServiceError> {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(ServiceError::Unavailable),
        sent = tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Accept, payload)) => {
            sent.unwrap_or(Err(ServiceError::TimedOut))
        }
    }
}

/// Sends `reject`, half-closes, and reports that nothing happened.
pub async fn reject<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    error: ServiceError,
) -> OperationReport {
    let _sent = tokio::time::timeout(FRAME_GRACE, async {
        let _sent = writer.reject(error).await;
        writer.shutdown().await
    })
    .await;
    OperationReport::new(OperationEnd::Rejected(error))
}

/// Sends one encoded terminal `outcome`, half-closes, and reports its class.
pub async fn outcome<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    (encoded, failure): (Result<Vec<u8>, ProtocolError>, Option<ServiceError>),
    (bytes_in, bytes_out): (u64, u64),
) -> OperationReport {
    let sent = match encoded {
        Ok(encoded) => {
            tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Outcome, &encoded))
                .await
                .unwrap_or(Err(ServiceError::TimedOut))
        }
        Err(_) => Err(ServiceError::Internal),
    };
    let _closed = tokio::time::timeout(FRAME_GRACE, writer.shutdown()).await;
    let end = match (sent, failure) {
        (Err(error), _) | (Ok(()), Some(error)) => OperationEnd::Failed(error),
        (Ok(()), None) => OperationEnd::Completed,
    };
    OperationReport::new(end).with_bytes(bytes_in, bytes_out)
}

/// Reports an operation that started but whose stream failed before a terminal frame.
pub(crate) const fn lost(
    error: ServiceError,
    (bytes_in, bytes_out): (u64, u64),
) -> OperationReport {
    OperationReport::new(OperationEnd::Failed(error)).with_bytes(bytes_in, bytes_out)
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

#[cfg(test)]
mod tests {
    use ma2a_core::service::{ServiceError, TransferResult};

    use crate::{FrameWriter, report::OperationEnd};

    #[tokio::test(start_paused = true)]
    async fn a_terminal_nobody_reads_is_given_up_after_the_grace() {
        // Given: a stream with no room, whose reader never drains it.
        let (_peer, host) = tokio::io::duplex(1);
        let mut writer = FrameWriter::new(host);
        let payload = TransferResult::encode_outcome(&Ok(TransferResult::new(0, 0, [0; 32])));

        // When
        let started = tokio::time::Instant::now();
        let report = super::outcome(&mut writer, (payload, None), (0, 0)).await;

        // Then
        assert_eq!(report.end, OperationEnd::Failed(ServiceError::TimedOut));
        assert!(started.elapsed() <= super::FRAME_GRACE * 2);
        let rejected = super::reject(&mut writer, ServiceError::Unavailable).await;
        assert_eq!(
            rejected.end,
            OperationEnd::Rejected(ServiceError::Unavailable)
        );
    }
}
