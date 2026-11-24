//! Caller side of Files v1: one bounded request per stream.

use ma2a_core::service::{
    Channel, FilesRequest, FilesResult, FrameKind, MAX_DATA_CHUNK_LEN, MAX_SMALL_FILE_LEN,
    ServiceError,
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{ClientError, First, decoded, failed, first, next, unexpected};
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
        First::Finished(frame) => return finished(&frame, request, Vec::new()),
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
            FrameKind::Outcome => return finished(&frame, request, read),
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
    let sent = if matches!(request, FilesRequest::Write(..)) {
        async {
            for chunk in content.chunks(MAX_DATA_CHUNK_LEN) {
                writer.data(Channel::Input, chunk).await?;
            }
            writer.end(Channel::Input).await?;
            writer.flush().await
        }
        .await
    } else {
        Ok(())
    };
    let frame = next(reader).await?;
    // A target that stopped reading still reports its terminal, which explains why; any other
    // answer to a body that was not sent in full is not believed.
    match sent {
        Err(error) if !(frame.kind() == FrameKind::Outcome && failed(&frame).is_err()) => {
            Err(ClientError::Stream(error))
        }
        _ => Ok(frame),
    }
}

fn finished(
    frame: &crate::Frame,
    request: &FilesRequest,
    read: Vec<u8>,
) -> Result<(FilesResult, Vec<u8>), ClientError> {
    let result = decoded(FilesResult::decode_outcome(frame.payload()))?;
    answers(request, &result, read.len()).map_err(ClientError::Stream)?;
    Ok((result, read))
}

/// Checks that `result` answers `request`: the operation's own result, the full write, and
/// content only for a read, exactly as long as the read reports.
fn answers(request: &FilesRequest, result: &FilesResult, read: usize) -> Result<(), ServiceError> {
    let counted = match (request, result) {
        (FilesRequest::Read(_, _, max), FilesResult::Read(length, _)) => {
            length <= max && usize::try_from(*length).ok() == Some(read)
        }
        (FilesRequest::Write(_, _, length), FilesResult::Written(size)) => {
            read == 0 && u64::from(*length) == *size
        }
        (FilesRequest::Stat(_), FilesResult::Metadata(_))
        | (FilesRequest::List(..), FilesResult::Listing(..))
        | (
            FilesRequest::Mkdir(..) | FilesRequest::Rename(..) | FilesRequest::Remove(..),
            FilesResult::Done,
        ) => read == 0,
        _ => return Err(ServiceError::InvalidInput),
    };
    if counted {
        Ok(())
    } else {
        Err(ServiceError::IntegrityMismatch)
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{FilesRequest, FilesResult, RemotePath, ServiceError, WriteMode};

    use super::answers;

    #[test]
    fn a_result_must_answer_its_own_request_in_full() -> Result<(), Box<dyn std::error::Error>> {
        let path = RemotePath::parse("docs:/a")?;
        let write = FilesRequest::Write(path.clone(), WriteMode::Replace, 3);
        let read = FilesRequest::Read(path.clone(), 0, 4);
        let stat = FilesRequest::Stat(path);

        assert_eq!(answers(&write, &FilesResult::Written(3), 0), Ok(()));
        assert_eq!(answers(&read, &FilesResult::Read(2, true), 2), Ok(()));
        assert_eq!(
            answers(&write, &FilesResult::Written(0), 0),
            Err(ServiceError::IntegrityMismatch)
        );
        assert_eq!(
            answers(&read, &FilesResult::Read(5, true), 5),
            Err(ServiceError::IntegrityMismatch)
        );
        assert_eq!(
            answers(&stat, &FilesResult::Done, 0),
            Err(ServiceError::InvalidInput)
        );
        assert_eq!(
            answers(&write, &FilesResult::Done, 0),
            Err(ServiceError::InvalidInput)
        );
        Ok(())
    }
}
