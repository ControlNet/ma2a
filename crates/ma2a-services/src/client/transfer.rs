//! Caller side of Transfer v1: verified downloads and committed uploads.

use ma2a_core::service::{
    Channel, ContentIdentity, FrameKind, MAX_DATA_CHUNK_LEN, ServiceError, TransferAccept,
    TransferCommit, TransferResult,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{FrameReader, FrameWriter};

/// Reads a download's `accept` and returns the identity of the content that will stream.
///
/// # Errors
/// Returns the refusal (`precondition_failed` when a resumed identity no longer matches),
/// or the stream or protocol failure.
pub async fn download_accepted<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<ContentIdentity, ClientError> {
    let First::Accepted(frame) = first(reader).await? else {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    };
    match TransferAccept::decode(frame.payload()) {
        Ok(TransferAccept::Get(identity)) => Ok(identity),
        Ok(TransferAccept::Put) | Err(_) => Err(ClientError::Stream(ServiceError::InvalidInput)),
    }
}

/// Streams download content into `sink`, extending `hasher`, and verifies the full hash.
///
/// `identity` is what the target accepted and `offset` where the stream starts, so exactly
/// `identity.size() - offset` bytes may arrive. `hasher` must already cover every byte before
/// `offset`, so the check spans the whole content exactly as the target hashed it.
///
/// # Errors
/// Returns the failed outcome (`content_changed` when the source moved underneath),
/// `integrity_mismatch` when the content runs past the accepted size or the terminal reports
/// another size, byte count, or hash than was received, a local write failure, or the stream
/// failure.
pub async fn download_content<R, S>(
    reader: &mut FrameReader<R>,
    (sink, hasher): (&mut S, &mut blake3::Hasher),
    (identity, offset): (ContentIdentity, u64),
) -> Result<TransferResult, ClientError>
where
    R: AsyncRead + Unpin,
    S: AsyncWrite + Unpin,
{
    let mismatch = || ClientError::Failed(ServiceError::IntegrityMismatch);
    let expected = identity.size().checked_sub(offset).ok_or_else(mismatch)?;
    let mut received = 0_u64;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Output, bytes) = frame.data().map_err(ClientError::Stream)? else {
                    return Err(ClientError::Stream(ServiceError::InvalidInput));
                };
                received = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|len| received.checked_add(len))
                    .filter(|total| *total <= expected)
                    .ok_or_else(mismatch)?;
                sink.write_all(bytes).await?;
                hasher.update(bytes);
            }
            FrameKind::Outcome => {
                let result = decoded(TransferResult::decode_outcome(frame.payload()))?;
                sink.flush().await?;
                return if result.bytes == received
                    && received == expected
                    && result.size == identity.size()
                    && result.hash == *hasher.finalize().as_bytes()
                {
                    Ok(result)
                } else {
                    Err(mismatch())
                };
            }
            _ => return Err(unexpected(&frame)),
        }
    }
}

/// Uploads exactly `size` bytes from `source` and returns the verified committed result.
///
/// # Errors
/// Returns the refusal or failed outcome, `integrity_mismatch` when the target committed a
/// different hash, a local read failure or short source (after cancelling the upload), or
/// the stream failure.
pub async fn upload<R, W, S>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (source, size): (&mut S, u64),
) -> Result<TransferResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    S: AsyncRead + Unpin,
{
    let frame = match first(reader).await? {
        // A replayed upload answers with its outcome at once; it must still be this content.
        First::Finished(frame) => {
            let result = decoded(TransferResult::decode_outcome(frame.payload()))?;
            let hash = hash_source((source, size))
                .await
                .map_err(ClientError::Local)?;
            return matching(result, (size, hash));
        }
        First::Accepted(frame) => frame,
    };
    if TransferAccept::decode(frame.payload()) != Ok(TransferAccept::Put) {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    }
    // `Some` holds the hash of everything sent; `None` is a broken stream, after which
    // the target can only report failure because it never received a commitment.
    let local = match send_content(writer, (source, size)).await {
        Ok(Ok(hash)) => {
            let committed = async {
                let commit = TransferCommit::new(hash)
                    .encode()
                    .map_err(|_| ServiceError::Internal)?;
                writer.frame(FrameKind::Control, &commit).await?;
                writer.flush().await
            };
            // A commitment that was not sent is a broken stream too: a target that stopped
            // reading still reports its terminal, which explains why.
            let committed: Result<(), ServiceError> = committed.await;
            committed.ok().map(|()| hash)
        }
        Ok(Err(error)) => {
            // A cancelled upload publishes nothing and may end without a terminal frame.
            crate::client::cancel_or_terminal(reader, writer).await;
            return Err(ClientError::Local(error));
        }
        Err(_stream) => None,
    };
    let frame = next(reader).await?;
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(&frame));
    }
    let result = decoded(TransferResult::decode_outcome(frame.payload()))?;
    let hash = local.ok_or(ClientError::Failed(ServiceError::IntegrityMismatch))?;
    matching(result, (size, hash))
}

/// Accepts a committed result only for exactly `size` bytes with `hash`, every one received.
fn matching(
    result: TransferResult,
    (size, hash): (u64, [u8; 32]),
) -> Result<TransferResult, ClientError> {
    if result.size == size && result.bytes == size && result.hash == hash {
        Ok(result)
    } else {
        Err(ClientError::Failed(ServiceError::IntegrityMismatch))
    }
}

/// Hashes exactly `size` bytes of `source`; a shorter source is an error.
async fn hash_source<S: AsyncRead + Unpin>(
    (source, size): (&mut S, u64),
) -> std::io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    let mut remaining = size;
    while remaining > 0 {
        let want = usize::try_from(remaining.min(MAX_DATA_CHUNK_LEN as u64)).unwrap_or(0);
        let chunk = buffer.get_mut(..want).unwrap_or_default();
        let read = source.read(chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        hasher.update(chunk.get(..read).unwrap_or_default());
        remaining -= u64::try_from(read).unwrap_or(remaining);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Sends the content and `end`; the outer error is a stream failure, the inner a local one.
async fn send_content<W, S>(
    writer: &mut FrameWriter<W>,
    (source, size): (&mut S, u64),
) -> Result<Result<[u8; 32], std::io::Error>, ServiceError>
where
    W: AsyncWrite + Unpin,
    S: AsyncRead + Unpin,
{
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    let mut remaining = size;
    while remaining > 0 {
        let want = usize::try_from(remaining.min(MAX_DATA_CHUNK_LEN as u64)).unwrap_or(0);
        let chunk = buffer.get_mut(..want).unwrap_or_default();
        let read = match source.read(chunk).await {
            Ok(0) => return Ok(Err(std::io::ErrorKind::UnexpectedEof.into())),
            Ok(read) => read,
            Err(error) => return Ok(Err(error)),
        };
        let bytes = chunk.get(..read).unwrap_or_default();
        hasher.update(bytes);
        writer.data(Channel::Input, bytes).await?;
        remaining -= u64::try_from(read).unwrap_or(remaining);
    }
    writer.end(Channel::Input).await?;
    Ok(Ok(*hasher.finalize().as_bytes()))
}
