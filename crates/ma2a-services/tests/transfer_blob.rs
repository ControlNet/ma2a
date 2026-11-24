//! A content-addressed blob is served only while it is still the content recorded.

#[path = "support/host.rs"]
mod support;

use std::fs;

use ma2a_core::service::{FrameKind, RemotePath, ServiceError, TransferResult};
use ma2a_services::serve_blob;
use support::{TempRoot, TestResult, content, drain, pipe_with};

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Serves `blob` as content recorded with `(size, hash)` and returns the frames sent.
async fn serve(
    root: &TempRoot,
    recorded: (u64, [u8; 32]),
) -> TestResult<Vec<ma2a_services::Frame>> {
    let (client, (mut reader, mut writer)) = pipe_with(1 << 20);
    let root = root.path().to_path_buf();
    let path = RemotePath::parse("artifacts:/blob")?;
    let server = tokio::spawn(async move {
        serve_blob(
            root,
            ((path, 0, None), recorded),
            (&mut reader, &mut writer),
        )
        .await
    });
    let (mut frames_in, _writer) = client;
    let frames = drain(&mut frames_in).await?;
    server.await?;
    Ok(frames)
}

#[tokio::test]
async fn a_blob_is_served_only_while_it_is_the_recorded_content() -> TestResult {
    let root = TempRoot::new("blob")?;
    let recorded = content(70_000);
    let size = u64::try_from(recorded.len())?;

    // The recorded content is served and verified.
    fs::write(root.path().join("blob"), &recorded)?;
    let frames = serve(&root, (size, hash(&recorded))).await?;
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    let result = TransferResult::decode_outcome(last.payload())?;
    assert_eq!(result.map(|result| result.size), Ok(size));

    // Same length, other bytes: the outcome fails instead of vouching for them.
    let mut corrupted = recorded.clone();
    if let Some(byte) = corrupted.get_mut(1_000) {
        *byte ^= 0xff;
    }
    fs::write(root.path().join("blob"), &corrupted)?;
    let frames = serve(&root, (size, hash(&recorded))).await?;
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(
        TransferResult::decode_outcome(last.payload())?,
        Err(ServiceError::IntegrityMismatch)
    );

    // Truncated: refused before anything is sent.
    fs::write(
        root.path().join("blob"),
        recorded.get(..100).ok_or("prefix")?,
    )?;
    let frames = serve(&root, (size, hash(&recorded))).await?;
    assert_eq!(
        frames.first().ok_or("reject")?.rejection()?,
        ServiceError::IntegrityMismatch
    );
    Ok(())
}
