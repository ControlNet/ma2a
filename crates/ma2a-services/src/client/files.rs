//! Caller side of Files v1: one bounded request per stream.

use ma2a_core::service::{
    Channel, FilesRequest, FilesResult, FrameKind, MAX_DATA_CHUNK_LEN, MAX_SMALL_FILE_LEN,
    ServiceError,
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{FrameReader, FrameWriter};

/// Runs one Files request whose `open` was written, returning its result and any content read.
///
/// `content` is the exact body of a `write`; every other operation passes an empty slice.
///
/// # Errors
/// Returns the refusal, the failed outcome, or the stream or protocol failure.
pub async fn run<R, W>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    request: &FilesRequest,
    content: &[u8],
) -> Result<(FilesResult, Vec<u8>), ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let frame = match first(reader).await? {
        First::Finished(frame) => return finished(&frame, Vec::new()),
        First::Accepted(_) => next_after_accept(reader, writer, (request, content)).await?,
    };
    let mut read = Vec::new();
    let mut frame = frame;
    loop {
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Output, bytes) = frame.data().map_err(ClientError::Stream)? else {
                    return Err(ClientError::Stream(ServiceError::InvalidInput));
                };
                if read.len().saturating_add(bytes.len()) > MAX_SMALL_FILE_LEN as usize {
                    return Err(ClientError::Stream(ServiceError::TooLarge));
                }
                read.extend_from_slice(bytes);
            }
            FrameKind::Outcome => return finished(&frame, read),
            _ => return Err(unexpected(&frame)),
        }
        frame = next(reader).await?;
    }
}

/// Sends a `write` body after `accept` and returns the next target frame.
async fn next_after_accept<R, W>(
    reader: &mut FrameReader<R>,
    writer: &mut FrameWriter<W>,
    (request, content): (&FilesRequest, &[u8]),
) -> Result<crate::Frame, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if matches!(request, FilesRequest::Write(..)) {
        let sent = async {
            for chunk in content.chunks(MAX_DATA_CHUNK_LEN) {
                writer.data(Channel::Input, chunk).await?;
            }
            writer.end(Channel::Input).await?;
            writer.flush().await
        };
        // A target that stopped reading still reports its terminal, which explains why.
        let _sent: Result<(), ServiceError> = sent.await;
    }
    next(reader).await
}

fn finished(frame: &crate::Frame, read: Vec<u8>) -> Result<(FilesResult, Vec<u8>), ClientError> {
    let result = decoded(FilesResult::decode_outcome(frame.payload()))?;
    if let FilesResult::Read(length, _) = result
        && usize::try_from(length).ok() != Some(read.len())
    {
        return Err(ClientError::Stream(ServiceError::IntegrityMismatch));
    }
    Ok((result, read))
}
