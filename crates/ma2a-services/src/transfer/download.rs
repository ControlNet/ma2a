//! Download: identity check, full-content BLAKE3, bounded chunks, change detection.

use std::{
    fs::{File, Metadata},
    io::Read as _,
    path::PathBuf,
    time::UNIX_EPOCH,
};

use ma2a_core::service::{
    Channel, ContentIdentity, MAX_DATA_CHUNK_LEN, RemotePath, ServiceError, TransferAccept,
    TransferResult,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    FrameReader, FrameWriter, files,
    reply::{self, STALL_DEADLINE},
    report::{OperationReport, len64},
};

/// Returns the observable identity of the open source content.
pub(crate) fn identity(metadata: &Metadata) -> ContentIdentity {
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        });
    ContentIdentity::new(metadata.len(), modified_ns)
}

struct Source {
    file: File,
    hasher: blake3::Hasher,
    buffer: Vec<u8>,
    offset: u64,
    size: u64,
    sent: u64,
}

impl Source {
    /// Reads the next chunk into the buffer, returning its length (0 at end of file).
    fn fill(mut self, limit: u64) -> Result<(Self, usize), ServiceError> {
        let wanted =
            usize::try_from(limit.min(len64(MAX_DATA_CHUNK_LEN))).unwrap_or(MAX_DATA_CHUNK_LEN);
        self.buffer.resize(wanted, 0);
        let read = crate::fs_error::map(self.file.read(&mut self.buffer))?;
        self.buffer.truncate(read);
        self.hasher.update(&self.buffer);
        Ok((self, read))
    }
}

async fn fill(source: Source, limit: u64) -> Result<(Source, usize), ServiceError> {
    tokio::task::spawn_blocking(move || source.fill(limit))
        .await
        .unwrap_or(Err(ServiceError::Internal))
}

/// The size and BLAKE3 hash a content-addressed file must have.
pub(super) type Content = (u64, [u8; 32]);

/// Serves one Get; with `content`, the file must be exactly that size and hash, as a
/// content-addressed blob is, and anything else fails with `integrity_mismatch`.
pub(super) async fn serve<R, W>(
    root: PathBuf,
    (path, offset, expected, content): (RemotePath, u64, Option<ContentIdentity>, Option<Content>),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let opened = tokio::task::spawn_blocking(move || files::open_regular(&root, &path))
        .await
        .unwrap_or(Err(ServiceError::Internal));
    let (file, metadata) = match opened {
        Ok(opened) => opened,
        Err(error) => return reply::reject(writer, error).await,
    };
    let current = identity(&metadata);
    if expected.is_some_and(|expected| expected != current) {
        return reply::reject(writer, ServiceError::PreconditionFailed).await;
    }
    if content.is_some_and(|(size, _)| size != current.size()) {
        return reply::reject(writer, ServiceError::IntegrityMismatch).await;
    }
    if offset > current.size() {
        return reply::reject(writer, ServiceError::InvalidInput).await;
    }
    let Ok(accept) = TransferAccept::Get(current).encode() else {
        return reply::reject(writer, ServiceError::Internal).await;
    };
    if let Err(error) = reply::accept_within_grace(writer, &accept).await {
        return reply::lost(error, (0, 0));
    }
    let source = Source {
        file,
        hasher: blake3::Hasher::new(),
        buffer: Vec::with_capacity(MAX_DATA_CHUNK_LEN),
        offset,
        size: current.size(),
        sent: 0,
    };
    let sent = std::sync::atomic::AtomicU64::new(0);
    let streamed = tokio::select! {
        result = stream(source, writer, &sent) => result,
        error = reply::client_cancelled(reader) => Err(error),
    };
    let sent = sent.into_inner();
    let result = match streamed {
        Ok((_, hash)) if content.is_some_and(|(_, recorded)| recorded != hash) => {
            Err(ServiceError::IntegrityMismatch)
        }
        Ok((source, hash)) => verify_unchanged(source, current).map(|()| hash),
        Err(error) => Err(error),
    };
    let summary = result.map(|hash| TransferResult::new(sent, current.size(), hash));
    let failure = summary.as_ref().err().copied();
    if matches!(failure, Some(ServiceError::Cancelled)) {
        return reply::lost(ServiceError::Cancelled, (0, sent));
    }
    reply::outcome(
        writer,
        (TransferResult::encode_outcome(&summary), failure),
        (0, sent),
    )
    .await
}

/// Hashes the skipped prefix, then streams the rest; a short read means the source shrank.
///
/// `sent` is shared with the caller so a cancelled stream still reports its progress.
async fn stream<W: AsyncWrite + Unpin>(
    mut source: Source,
    writer: &mut FrameWriter<W>,
    sent: &std::sync::atomic::AtomicU64,
) -> Result<(Source, [u8; 32]), ServiceError> {
    let (offset, size) = (source.offset, source.size);
    let mut position = 0_u64;
    while position < size {
        let (next, read) = fill(source, size - position).await?;
        source = next;
        if read == 0 {
            return Err(ServiceError::ContentChanged);
        }
        let chunk_start = position;
        position += len64(read);
        if position <= offset {
            continue;
        }
        let skip = usize::try_from(offset.saturating_sub(chunk_start)).unwrap_or(0);
        let bytes = source.buffer.get(skip..).ok_or(ServiceError::Internal)?;
        tokio::time::timeout(STALL_DEADLINE, writer.data(Channel::Output, bytes))
            .await
            .map_err(|_| ServiceError::TimedOut)??;
        source.sent += len64(bytes.len());
        sent.store(source.sent, std::sync::atomic::Ordering::Relaxed);
    }
    let hash = *source.hasher.finalize().as_bytes();
    Ok((source, hash))
}

fn verify_unchanged(mut source: Source, expected: ContentIdentity) -> Result<(), ServiceError> {
    let mut probe = [0_u8; 1];
    let grew = crate::fs_error::map(source.file.read(&mut probe))? > 0;
    let now = crate::fs_error::map(source.file.metadata())?;
    if grew || identity(&now) != expected {
        Err(ServiceError::ContentChanged)
    } else {
        Ok(())
    }
}
