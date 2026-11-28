use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use ma2a_core::service::{
    Channel, ContentIdentity, FrameKind, ServiceError, TransferAccept, TransferResult,
};

use super::{Partial, partial_path};
use crate::{
    FrameReader, FrameWriter,
    client::{ClientError, transfer},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> std::io::Result<Self> {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-resume-{label}-{}-{id}", std::process::id()));
        let _removed = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

async fn scripted_stream(content: &[u8]) -> Result<Vec<u8>, ServiceError> {
    let mut bytes = Vec::new();
    let identity = ContentIdentity::new(u64::try_from(content.len()).unwrap_or(0), 1);
    let mut writer = FrameWriter::new(&mut bytes);
    let accept = TransferAccept::Get(identity)
        .encode()
        .map_err(|_| ServiceError::Internal)?;
    writer.frame(FrameKind::Accept, &accept).await?;
    writer.data(Channel::Output, content).await?;
    let hash = *blake3::hash(content).as_bytes();
    let result = TransferResult::new(identity.size(), identity.size(), hash);
    let outcome =
        TransferResult::encode_outcome(&Ok(result)).map_err(|_| ServiceError::Internal)?;
    writer.frame(FrameKind::Outcome, &outcome).await?;
    writer.flush().await?;
    Ok(bytes)
}

async fn drive_download(
    partial: &mut Partial,
    stream: &[u8],
) -> Result<TransferResult, ClientError> {
    let mut reader = FrameReader::new(stream);
    let identity = transfer::download_accepted(&mut reader).await?;
    partial.begin(identity).await?;
    let offset = partial.offset();
    transfer::download_content(&mut reader, partial.sink(), (identity, offset)).await
}

#[tokio::test]
async fn directory_sync_error_prevents_first_content_write() -> TestResult {
    let payload = b"body-must-not-be-written";
    let stream = scripted_stream(payload).await?;

    for fail_parent in [false, true] {
        let label = if fail_parent { "parent" } else { "helper" };
        let dir = TempDir::new(label)?;
        let destination = dir.path().join("report");
        let mut partial = Partial::open(&destination, "peer docs:/report", false).await?;
        if fail_parent {
            partial.helpers.parent.0.set_fail_sync();
        } else {
            partial.helpers.directory.set_fail_sync();
        }

        let result = drive_download(&mut partial, &stream).await;
        assert!(
            result.is_err(),
            "{label} directory sync error must fail before writing content"
        );
        assert_eq!(partial.identity(), None, "{label}");
        assert_eq!(
            std::fs::metadata(partial_path(&destination)?)?.len(),
            0,
            "{label}: partial file must remain empty when directory sync fails"
        );
        partial.discard().await?;
    }
    Ok(())
}

#[tokio::test]
async fn fresh_destination_downloads_with_directory_syncs() -> TestResult {
    let dir = TempDir::new("fresh-sync")?;
    let destination = dir.path().join("report");
    let payload = b"durable-sidecar-before-body";
    let stream = scripted_stream(payload).await?;

    let mut partial = Partial::open(&destination, "peer docs:/report", false).await?;
    let result = drive_download(&mut partial, &stream).await?;
    assert_eq!(result.bytes, u64::try_from(payload.len()).unwrap_or(0));
    partial.finish(false).await?;
    assert_eq!(std::fs::read(&destination)?, payload);
    Ok(())
}
