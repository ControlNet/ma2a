//! Target-side Transfer v1: BLAKE3-verified downloads and staged, verified uploads.

mod download;
mod upload;

use std::{future::Future, path::PathBuf};

use ma2a_core::{
    RequestId,
    service::{ContentIdentity, RemotePath, ServiceError, TransferRequest, TransferResource},
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{FrameReader, FrameWriter, report::OperationReport};

/// Durable record of upload staging files, written before a staging file exists.
///
/// A Runtime restart removes every recorded file, so no staging file outlives its operation.
pub trait StagingJournal: Send + Sync {
    /// Durably records one staging path before it is created.
    fn record(&self, path: &str) -> impl Future<Output = Result<(), ServiceError>> + Send;

    /// Forgets one staging path after its file was finalized or removed.
    fn forget(&self, path: &str) -> impl Future<Output = ()> + Send;
}

/// One Get of a content-addressed file: its path, resume offset and expected identity.
pub type BlobGet = (RemotePath, u64, Option<ContentIdentity>);

/// Serves a Get of one content-addressed file, which must be exactly `content`.
///
/// `content` is its size and BLAKE3 hash. A file that differs fails with `integrity_mismatch`
/// instead of being passed on as if it were the content recorded.
pub async fn serve_blob<R, W>(
    root: PathBuf,
    ((path, offset, expected), content): (BlobGet, (u64, [u8; 32])),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    download::serve(
        root,
        (path, offset, expected, Some(content)),
        (reader, writer),
    )
    .await
}

/// Serves one Transfer request inside the canonical local directory of its exported root.
pub async fn serve_transfer<R, W, J>(
    (root, request_id): (PathBuf, RequestId),
    (request, journal): (TransferRequest, &J),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    J: StagingJournal,
{
    match request {
        TransferRequest::Get(TransferResource::File(path), offset, expected) => {
            download::serve(root, (path, offset, expected, None), (reader, writer)).await
        }
        TransferRequest::Put(TransferResource::File(path), size, mode) => {
            let target = upload::Target {
                root,
                path,
                size,
                mode,
                request_id,
            };
            upload::serve(target, journal, (reader, writer)).await
        }
    }
}
