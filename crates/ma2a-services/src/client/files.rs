//! Caller side of Files v1: one bounded request per stream.

use ma2a_core::service::{
    Channel, DirEntry, FilesRequest, FilesResult, FrameKind, MAX_DATA_CHUNK_LEN,
    MAX_SMALL_FILE_LEN, ServiceError,
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{ClientError, First, decoded, empty_accept, failed, first, next, unexpected};
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
    let reads = matches!(request, FilesRequest::Read(..));
    let frame = match first(reader).await? {
        // A read is always accepted before its content, and reserves no RequestId whose
        // outcome replay could answer alone.
        First::Finished(_) if reads => {
            return Err(ClientError::Stream(ServiceError::InvalidInput));
        }
        First::Finished(frame) => return finished(&frame, request, Vec::new()),
        // Only a read or a write is accepted; every other operation answers alone.
        First::Accepted(frame) if reads || matches!(request, FilesRequest::Write(..)) => {
            empty_accept(&frame)?;
            next_after_accept(reader, writer, (request, content)).await?
        }
        First::Accepted(_) => return Err(ClientError::Stream(ServiceError::InvalidInput)),
    };
    let mut read = Vec::new();
    let mut frame = frame;
    loop {
        match frame.kind() {
            // Only a read carries content.
            FrameKind::Data if reads => {
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
        (FilesRequest::List(_, after), FilesResult::Listing(entries, next)) if read == 0 => {
            return continues(after.as_deref(), entries, next.as_deref());
        }
        (FilesRequest::List(..), FilesResult::Listing(..)) => false,
        (FilesRequest::Stat(_), FilesResult::Metadata(_))
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

/// Checks that a listing page continues strictly past the cursor sent: its names in strict byte
/// order after `after`, and the cursor it names next past `after` and not behind its last name.
/// A cursor past every name, even on an empty page, names entries the host left out. Any other
/// page would repeat an entry, or the whole request, on the next page.
fn continues(
    after: Option<&str>,
    entries: &[DirEntry],
    next: Option<&str>,
) -> Result<(), ServiceError> {
    let mut reached = after;
    for entry in entries {
        if reached.is_some_and(|reached| entry.name.as_str() <= reached) {
            return Err(ServiceError::InvalidInput);
        }
        reached = Some(&entry.name);
    }
    let behind = next.is_some_and(|next| {
        entries.last().map_or_else(
            || after.is_some_and(|after| next <= after),
            |last| next < last.name.as_str(),
        )
    });
    if behind {
        Err(ServiceError::InvalidInput)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{
        DirEntry, FileKind, FileMetadata, FilesRequest, FilesResult, RemotePath, ServiceError,
        WriteMode,
    };

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

    /// Whether a page listing `listed` and naming `next` answers a list after `after`.
    fn page(after: Option<&str>, listed: &[&str], next: Option<&str>) -> Result<(), ServiceError> {
        let path = RemotePath::parse("docs:/").map_err(|_| ServiceError::Internal)?;
        let request = FilesRequest::List(path, after.map(str::to_owned));
        let entries = listed
            .iter()
            .map(|name| DirEntry::new((*name).to_owned(), FileMetadata::new(FileKind::File, 0)))
            .collect();
        answers(
            &request,
            &FilesResult::Listing(entries, next.map(str::to_owned)),
            0,
        )
    }

    #[test]
    fn a_listing_page_must_continue_strictly_past_the_cursor_sent() {
        assert_eq!(page(None, &["b", "c"], Some("c")), Ok(()));
        assert_eq!(page(Some("b"), &["c", "d"], Some("d")), Ok(()));
        assert_eq!(page(Some("b"), &["c"], None), Ok(()));
        // The cursor may name an entry the host left out, also on an otherwise empty page.
        assert_eq!(page(Some("b"), &["c"], Some("e")), Ok(()));
        assert_eq!(page(Some("b"), &[], Some("c")), Ok(()));
        assert_eq!(page(None, &[], Some("c")), Ok(()));
        let malformed = Err(ServiceError::InvalidInput);
        // Entries out of order or repeated, even on a first page.
        assert_eq!(page(None, &["c", "b"], None), malformed);
        assert_eq!(page(Some("a"), &["c", "c"], None), malformed);
        // An entry or a cursor not past the cursor sent, such as the same cursor again.
        assert_eq!(page(Some("b"), &["b"], None), malformed);
        assert_eq!(page(Some("b"), &[], Some("b")), malformed);
        assert_eq!(page(Some("b"), &[], Some("a")), malformed);
        // A cursor behind the last entry listed would list that entry again on the next page.
        assert_eq!(page(Some("a"), &["c"], Some("b")), malformed);
        assert_eq!(page(None, &["c", "d"], Some("c")), malformed);
    }
}
