//! Target-side Files v1 over one authorized frame stream.
//!
//! Content-free operations answer with `reject` (nothing happened) or `outcome`. Small reads
//! and writes send `accept` first and carry their content in `data` frames.

mod commit;
mod content;
mod entry;
mod ops;
#[cfg(unix)]
#[path = "files/dir_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "files/dir_windows.rs"]
mod platform;
#[cfg(windows)]
pub(crate) mod rename_windows;
mod resolve;
mod stop;

use std::path::PathBuf;

use ma2a_core::service::{
    Channel, EMPTY_ACCEPT, FilesRequest, FilesResult, FrameKind, MAX_SMALL_FILE_LEN, ServiceError,
    WriteMode,
};
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) use commit::TempFile;
pub(crate) use content::open_regular;
pub(crate) use resolve::destination;
pub use resolve::is_windows_reserved;

use crate::{
    FrameReader, FrameWriter, reply,
    report::{OperationReport, len64},
};
use ops::Failure;
use stop::Stop;

/// Serves one Files request inside the canonical local directory of its exported root.
pub async fn serve_files<R, W>(
    root: PathBuf,
    request: FilesRequest,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    match request {
        FilesRequest::Read(path, offset, length) => {
            let result = blocking(move |_| content::read(&root, &path, (offset, length))).await;
            read_reply(result, writer).await
        }
        FilesRequest::Write(path, mode, length) => {
            write_reply((root, path, mode, length), reader, writer).await
        }
        request => {
            // A cancel or reset stops the job; what it already did may stand, so the answer
            // is a failed outcome rather than a refusal.
            let result = tokio::select! {
                result = blocking(move |stop| content_free(&root, &request, stop)) => result,
                error = reply::client_cancelled(reader) => Err(Failure { error, partial: true }),
            };
            content_free_reply(result, writer).await
        }
    }
}

fn content_free(
    root: &std::path::Path,
    request: &FilesRequest,
    stop: &Stop,
) -> Result<FilesResult, Failure> {
    match request {
        FilesRequest::Stat(path) => Ok(ops::stat(root, path)?),
        FilesRequest::List(path, cursor) => Ok(ops::list(root, path, (cursor.as_deref(), stop))?),
        FilesRequest::Mkdir(path, parents) => ops::mkdir(root, path, *parents),
        FilesRequest::Rename(from, to, replace) => Ok(ops::rename(root, (from, to), *replace)?),
        FilesRequest::Remove(path, recursive) => ops::remove(root, path, (*recursive, stop)),
        FilesRequest::Read(..) | FilesRequest::Write(..) => Err(ServiceError::Internal.into()),
    }
}

/// Most blocking Files jobs at once across the Runtime.
///
/// A job keeps its permit even when its stream is reset at the deadline, so a caller cannot
/// pile up detached work such as a huge recursive removal or listing.
const MAX_BLOCKING_JOBS: usize = 16;
static BLOCKING_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_BLOCKING_JOBS);

/// Runs `work` on a blocking thread. Once this future is dropped before the job ends, as when
/// the stream is cancelled or its grant revoked, `work` sees its [`Stop`] set.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce(&Stop) -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let stop = Stop::default();
    let _stops = stop.on_drop();
    let Ok(permit) = BLOCKING_JOBS.acquire().await else {
        return Err(ServiceError::Internal.into());
    };
    tokio::task::spawn_blocking(move || {
        let _held = permit;
        work(&stop)
    })
    .await
    .unwrap_or_else(|_| Err(ServiceError::Internal.into()))
}

async fn content_free_reply<W: AsyncWrite + Unpin>(
    result: Result<FilesResult, Failure>,
    writer: &mut FrameWriter<W>,
) -> OperationReport {
    match result {
        Ok(result) => outcome(writer, Ok(result), (0, 0)).await,
        Err(Failure {
            error,
            partial: true,
        }) => outcome(writer, Err(error), (0, 0)).await,
        Err(Failure { error, .. }) => reply::reject(writer, error).await,
    }
}

async fn read_reply<W: AsyncWrite + Unpin>(
    result: Result<(Vec<u8>, u64, u64), Failure>,
    writer: &mut FrameWriter<W>,
) -> OperationReport {
    let (bytes, size, offset) = match result {
        Ok(read) => read,
        Err(Failure { error, .. }) => return reply::reject(writer, error).await,
    };
    let sent = async {
        reply::accept_within_grace(writer, &EMPTY_ACCEPT).await?;
        if !bytes.is_empty() {
            tokio::time::timeout(reply::STALL_DEADLINE, writer.data(Channel::Output, &bytes))
                .await
                .unwrap_or(Err(ServiceError::TimedOut))?;
        }
        Ok::<(), ServiceError>(())
    };
    if let Err(error) = sent.await {
        return reply::lost(error, (0, 0));
    }
    let length = len64(bytes.len());
    let eof = offset.saturating_add(length) >= size;
    let length = u32::try_from(bytes.len()).unwrap_or(MAX_SMALL_FILE_LEN);
    outcome(
        writer,
        Ok(FilesResult::Read(length, eof)),
        (0, len64(bytes.len())),
    )
    .await
}

async fn write_reply<R, W>(
    (root, path, mode, length): (PathBuf, ma2a_core::service::RemotePath, WriteMode, u32),
    reader: &mut FrameReader<R>,
    writer: &mut FrameWriter<W>,
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let prepared = blocking(move |_| Ok(content::prepare_write(&root, &path, mode)?)).await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(Failure { error, .. }) => return reply::reject(writer, error).await,
    };
    if let Err(error) = reply::accept_within_grace(writer, &EMPTY_ACCEPT).await {
        return reply::lost(error, (0, 0));
    }
    let content = match receive_content(reader, length).await {
        Ok(content) => content,
        Err(error) => return outcome(writer, Err(error), (0, 0)).await,
    };
    let received = len64(content.len());
    let written = blocking(move |_| Ok(content::commit_write(prepared, &content, mode)?)).await;
    let result = written
        .map(FilesResult::Written)
        .map_err(|failure| failure.error);
    outcome(writer, result, (received, 0)).await
}

/// Receives exactly `length` input bytes followed by `end` on the input channel.
pub(crate) async fn receive_content<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    length: u32,
) -> Result<Vec<u8>, ServiceError> {
    let expected = usize::try_from(length).map_err(|_| ServiceError::TooLarge)?;
    let mut content = Vec::with_capacity(expected);
    loop {
        let frame = reader.expect().await?;
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                if content.len().saturating_add(bytes.len()) > expected {
                    return Err(ServiceError::TooLarge);
                }
                content.extend_from_slice(bytes);
            }
            FrameKind::End if frame.end()? == Channel::Input => {
                return if content.len() == expected {
                    Ok(content)
                } else {
                    Err(ServiceError::IntegrityMismatch)
                };
            }
            FrameKind::Cancel => return Err(ServiceError::Cancelled),
            _ => return Err(ServiceError::InvalidInput),
        }
    }
}

async fn outcome<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    result: Result<FilesResult, ServiceError>,
    bytes: (u64, u64),
) -> OperationReport {
    let encoded = FilesResult::encode_outcome(&result);
    reply::outcome(writer, (encoded, result.err()), bytes).await
}
