//! The upload client accepts a committed result only for the very content it holds.

#[path = "support/host.rs"]
mod support;

use ma2a_core::service::{Channel, FrameKind, ServiceError, TransferAccept, TransferResult};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{self, ClientError},
};
use support::{TestResult, pipe};

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Uploads `source` to a target that answers `result`, after accepting and reading the content
/// when `accept` is set, or at once as a replayed request does.
async fn upload(
    source: &'static [u8],
    (accept, result): (bool, TransferResult),
) -> TestResult<Result<TransferResult, ClientError>> {
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
    let host = tokio::spawn(async move {
        if accept {
            let put = TransferAccept::Put
                .encode()
                .map_err(|_| ServiceError::Internal)?;
            host_writer.frame(FrameKind::Accept, &put).await?;
            // Content, its end, then the commitment.
            while let Some(frame) = host_reader.next().await? {
                if frame.kind() == FrameKind::Control {
                    break;
                }
            }
        }
        let outcome =
            TransferResult::encode_outcome(&Ok(result)).map_err(|_| ServiceError::Internal)?;
        host_writer.frame(FrameKind::Outcome, &outcome).await?;
        host_writer.flush().await
    });
    let size = u64::try_from(source.len())?;
    let uploaded =
        client::transfer::upload((&mut reader, &mut writer), (&mut &source[..], size)).await;
    let _answered = host.await?;
    Ok(uploaded)
}

const fn mismatched(result: &Result<TransferResult, ClientError>) -> bool {
    matches!(
        result,
        Err(ClientError::Failed(ServiceError::IntegrityMismatch))
    )
}

#[tokio::test]
async fn a_result_for_other_content_is_refused() -> TestResult {
    let honest = TransferResult::new(5, 5, hash(b"hello"));

    // Accepted, streamed, and committed as sent.
    assert!(upload(b"hello", (true, honest)).await?.is_ok());
    // Every byte claimed, but none received.
    let nothing = TransferResult::new(0, 5, hash(b"hello"));
    assert!(mismatched(&upload(b"hello", (true, nothing)).await?));

    // A replayed answer is checked against the local content too.
    assert!(upload(b"hello", (false, honest)).await?.is_ok());
    let other = TransferResult::new(999, 999, [7; 32]);
    assert!(mismatched(&upload(b"hello", (false, other)).await?));
    Ok(())
}

/// A pipe that takes `.0` more bytes and fails every write after them.
struct Budget(usize);

impl tokio::io::AsyncWrite for Budget {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.0 == 0 {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let taken = buffer.len().min(self.0);
        self.0 -= taken;
        std::task::Poll::Ready(Ok(taken))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// Uploads "hello" whose content and end are sent but whose commitment is not, to a target
/// that accepts it and then answers `outcome`.
async fn upload_without_commit(
    outcome: &Result<TransferResult, ServiceError>,
) -> TestResult<Result<TransferResult, ClientError>> {
    let mut content = FrameWriter::new(Vec::new());
    content.data(Channel::Input, b"hello").await?;
    content.end(Channel::Input).await?;
    content.flush().await?;
    let budget = content.into_inner().len();
    let mut replies = FrameWriter::new(Vec::new());
    let put = TransferAccept::Put
        .encode()
        .map_err(|_| ServiceError::Internal)?;
    replies.frame(FrameKind::Accept, &put).await?;
    let outcome = TransferResult::encode_outcome(outcome).map_err(|_| ServiceError::Internal)?;
    replies.frame(FrameKind::Outcome, &outcome).await?;
    replies.flush().await?;
    let replies = replies.into_inner();
    let io = (
        &mut FrameReader::new(replies.as_slice()),
        &mut FrameWriter::new(Budget(budget)),
    );
    Ok(client::transfer::upload(io, (&mut &b"hello"[..], 5)).await)
}

#[tokio::test]
async fn an_upload_whose_commitment_was_not_sent_never_reports_success() -> TestResult {
    // Given: a target that claims the upload committed although its commitment never left.
    let honest = TransferResult::new(5, 5, hash(b"hello"));
    let uploaded = upload_without_commit(&Ok(honest)).await?;

    // Then: nothing was committed, so the claim is not believed.
    assert!(mismatched(&uploaded), "{uploaded:?}");

    // And: a failure the target reports still explains why.
    let failed = upload_without_commit(&Err(ServiceError::Io)).await?;
    assert!(
        matches!(failed, Err(ClientError::Failed(ServiceError::Io))),
        "{failed:?}"
    );
    Ok(())
}
