//! Upload: journaled staging file, bounded chunks, size and BLAKE3 commit, atomic publish.

use std::path::PathBuf;

use ma2a_core::{
    RequestId,
    service::{
        Channel, FrameKind, RemotePath, ServiceError, TransferAccept, TransferCommit,
        TransferResult, WriteMode,
    },
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::StagingJournal;
use crate::{
    FrameReader, FrameWriter,
    files::{self, TempFile},
    reply::{self, STALL_DEADLINE},
    report::{OperationReport, hex, len64},
};

pub(super) struct Target {
    pub(super) root: PathBuf,
    pub(super) path: RemotePath,
    pub(super) size: u64,
    pub(super) mode: WriteMode,
    pub(super) request_id: RequestId,
}

/// Returns the staging name used for one upload request inside its destination directory.
pub(crate) fn staging_name(request_id: &RequestId) -> String {
    format!(".ma2a-upload-{}.part", hex(request_id.as_bytes()))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ServiceError> + Send + 'static,
) -> Result<T, ServiceError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(ServiceError::Internal))
}

pub(super) async fn serve<R, W, J>(
    target: Target,
    journal: &J,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    J: StagingJournal,
{
    let Target {
        root,
        path,
        size,
        mode,
        request_id,
    } = target;
    let resolved = blocking(move || {
        files::destination(&root, &path)
            .map(|(parent, name, existing)| (parent, name.to_owned(), existing))
    })
    .await;
    let (parent, destination, existing) = match resolved {
        Ok(resolved) => resolved,
        Err(error) => return reply::reject(writer, error).await,
    };
    if mode == WriteMode::CreateNew && existing.is_some() {
        return reply::reject(writer, ServiceError::AlreadyExists).await;
    }
    let name = staging_name(&request_id);
    let Some(staging) = parent.path().join(&name).to_str().map(str::to_owned) else {
        return reply::reject(writer, ServiceError::InvalidPath).await;
    };
    if let Err(error) = journal.record(&staging).await {
        return reply::reject(writer, error).await;
    }
    let created = blocking(move || TempFile::create(parent, &name)).await;
    let temporary = match created {
        Ok(temporary) => temporary,
        Err(error) => {
            journal.forget(&staging).await;
            return reply::reject(writer, error).await;
        }
    };
    let report = receive((temporary, destination, mode, size), (reader, writer)).await;
    journal.forget(&staging).await;
    report
}

/// One staged upload between `accept` and publication.
struct Upload {
    temporary: Option<TempFile>,
    size: u64,
    received: u64,
    hasher: blake3::Hasher,
}

async fn receive<R, W>(
    (temporary, destination, mode, size): (TempFile, String, WriteMode, u64),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let accepted = match TransferAccept::Put.encode() {
        Ok(accept) => writer.frame(FrameKind::Accept, &accept).await,
        Err(_) => Err(ServiceError::Internal),
    };
    let mut upload = Upload {
        temporary: Some(temporary),
        size,
        received: 0,
        hasher: blake3::Hasher::new(),
    };
    if let Err(error) = accepted {
        discard(upload.temporary.take()).await;
        return reply::lost(error, (0, 0));
    }
    let result = match content(&mut upload, reader).await {
        Ok(hash) => match upload.temporary.take() {
            Some(temporary) => blocking(move || temporary.finalize(&destination, mode))
                .await
                .map(|()| TransferResult::new(size, size, hash)),
            None => Err(ServiceError::Internal),
        },
        Err(error) => Err(error),
    };
    discard(upload.temporary.take()).await;
    let received = upload.received;
    let failure = result.as_ref().err().copied();
    if matches!(failure, Some(ServiceError::Cancelled)) {
        return reply::lost(ServiceError::Cancelled, (received, 0));
    }
    reply::outcome(
        writer,
        (TransferResult::encode_outcome(&result), failure),
        (received, 0),
    )
    .await
}

/// Receives exactly `size` bytes, `end`, and a matching commitment.
async fn content<R: AsyncRead + Unpin>(
    upload: &mut Upload,
    reader: &mut FrameReader<R>,
) -> Result<[u8; 32], ServiceError> {
    loop {
        let frame = tokio::time::timeout(STALL_DEADLINE, reader.expect())
            .await
            .map_err(|_| ServiceError::TimedOut)??;
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                if upload.received + len64(bytes.len()) > upload.size {
                    return Err(ServiceError::TooLarge);
                }
                upload.hasher.update(bytes);
                upload.received += len64(bytes.len());
                let bytes = frame.into_payload();
                let mut temporary = upload.temporary.take().ok_or(ServiceError::Internal)?;
                upload.temporary = Some(
                    blocking(move || {
                        temporary
                            .write(bytes.get(1..).unwrap_or_default())
                            .map(|()| temporary)
                    })
                    .await?,
                );
            }
            FrameKind::End if frame.end() == Ok(Channel::Input) => break,
            FrameKind::Cancel => return Err(ServiceError::Cancelled),
            _ => return Err(ServiceError::InvalidInput),
        }
    }
    let frame = tokio::time::timeout(STALL_DEADLINE, reader.expect())
        .await
        .map_err(|_| ServiceError::TimedOut)??;
    let commit = match frame.kind() {
        FrameKind::Control => {
            TransferCommit::decode(frame.payload()).map_err(|_| ServiceError::InvalidInput)?
        }
        FrameKind::Cancel => return Err(ServiceError::Cancelled),
        _ => return Err(ServiceError::InvalidInput),
    };
    let hash = *upload.hasher.finalize().as_bytes();
    if upload.received == upload.size && commit.hash() == hash {
        Ok(hash)
    } else {
        Err(ServiceError::IntegrityMismatch)
    }
}

/// Removes an unpublished staging file on a blocking thread.
async fn discard(temporary: Option<TempFile>) {
    if let Some(temporary) = temporary {
        let _removed = tokio::task::spawn_blocking(move || drop(temporary)).await;
    }
}
