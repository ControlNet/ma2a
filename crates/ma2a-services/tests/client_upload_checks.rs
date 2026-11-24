//! The upload client accepts a committed result only for the very content it holds.

#[path = "support/host.rs"]
mod support;

use ma2a_core::service::{FrameKind, ServiceError, TransferAccept, TransferResult};
use ma2a_services::client::{self, ClientError};
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
