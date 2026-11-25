//! The caller-side hosted-service driver against a scripted service end.
//!
//! The service end here is a test script writing exact Services v1 frames, not a service.

#[path = "support/host.rs"]
mod support;

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use ma2a_core::service::{
    Channel, DescribePage, DirEntry, EMPTY_ACCEPT, FileKind, FileMetadata, FilesOutcome,
    FilesRequest, FilesResult, FrameKind, LogWindow, ProcessAccept, ProcessReply, ProcessState,
    RemotePath, ServiceError, WriteMode, encode_call_accept, encode_call_outcome, failed_outcome,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{
        ClientError, files,
        hosted::{self, LogsRead},
        process::{self, ProcessControl, ProcessIo},
        service,
    },
};
use support::{TestResult, pipe};
use tokio::io::AsyncWrite;

async fn within<T>(future: impl std::future::Future<Output = T>) -> TestResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

const fn no_input() -> ProcessIo<&'static [u8], Vec<u8>, Vec<u8>> {
    ProcessIo {
        input: None,
        output: Vec::new(),
        error: Some(Vec::new()),
    }
}

#[tokio::test]
async fn an_unaccepted_reply_is_the_whole_answer() -> TestResult {
    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    service_writer
        .frame(FrameKind::Outcome, &encode_call_outcome(Ok(b"hello"))?)
        .await?;
    service_writer.flush().await?;
    let called = within(service::call(
        (&mut reader, &mut writer),
        no_input(),
        std::future::pending(),
    ))
    .await??;
    assert_eq!(called.header, None);
    assert_eq!(called.result, b"hello");
    Ok(())
}

#[tokio::test]
async fn an_accepted_call_relays_input_output_and_errors_until_its_outcome() -> TestResult {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let script = async {
        service_writer
            .frame(FrameKind::Accept, &encode_call_accept(b"header")?)
            .await?;
        service_writer.flush().await?;
        let input = service_reader.expect().await?;
        assert_eq!(input.data()?, (Channel::Input, &b"abc"[..]));
        assert_eq!(service_reader.expect().await?.end()?, Channel::Input);
        service_writer.data(Channel::Output, b"abc").await?;
        service_writer.frame(FrameKind::Control, b"ignored").await?;
        service_writer.data(Channel::Error, b"warning").await?;
        service_writer.end(Channel::Output).await?;
        service_writer
            .frame(FrameKind::Outcome, &encode_call_outcome(Ok(b"3"))?)
            .await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let local = ProcessIo {
        input: Some(&b"abc"[..]),
        output: &mut output,
        error: Some(&mut error),
    };
    let calling = service::call((&mut reader, &mut writer), local, std::future::pending());
    let (called, scripted) = within(async { tokio::join!(calling, script) }).await?;
    scripted?;
    let called = called?;
    assert_eq!(called.header.as_deref(), Some(&b"header"[..]));
    assert_eq!(called.result, b"3");
    assert_eq!(
        (output.as_slice(), error.as_slice()),
        (&b"abc"[..], &b"warning"[..])
    );
    Ok(())
}

#[tokio::test]
async fn a_cancelled_call_ends_with_the_services_failure() -> TestResult {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let script = async {
        service_writer
            .frame(FrameKind::Accept, &encode_call_accept(b"")?)
            .await?;
        service_writer.flush().await?;
        assert_eq!(service_reader.expect().await?.kind(), FrameKind::Cancel);
        let failed = encode_call_outcome(Err(ServiceError::Cancelled))?;
        service_writer.frame(FrameKind::Outcome, &failed).await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };
    let calling = service::call((&mut reader, &mut writer), no_input(), async {});
    let (called, scripted) = within(async { tokio::join!(calling, script) }).await?;
    scripted?;
    assert!(matches!(
        called,
        Err(ClientError::Failed(ServiceError::Cancelled))
    ));
    Ok(())
}

#[tokio::test]
async fn a_call_cancelled_before_any_answer_tells_a_unary_service() -> TestResult {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let script = async {
        // A unary service answers only with its outcome, so it must hear the cancel first.
        assert_eq!(service_reader.expect().await?.kind(), FrameKind::Cancel);
        let failed = encode_call_outcome(Err(ServiceError::Cancelled))?;
        service_writer.frame(FrameKind::Outcome, &failed).await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };
    let calling = service::call((&mut reader, &mut writer), no_input(), async {});
    let (called, scripted) = within(async { tokio::join!(calling, script) }).await?;
    scripted?;
    assert!(
        matches!(called, Err(ClientError::Failed(ServiceError::Cancelled))),
        "{called:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_process_cancelled_before_its_accept_is_cancelled_once() -> TestResult {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let script = async {
        assert_eq!(service_reader.expect().await?.kind(), FrameKind::Cancel);
        service_writer
            .frame(FrameKind::Accept, &EMPTY_ACCEPT)
            .await?;
        let failed = failed_outcome(ServiceError::Cancelled)?;
        service_writer.frame(FrameKind::Outcome, &failed).await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };
    let control = ProcessControl {
        resizes: None,
        cancel: async {},
    };
    let running = process::run_streamed((&mut reader, &mut writer), no_input(), control);
    let (ran, scripted) = within(async { tokio::join!(running, script) }).await?;
    scripted?;
    assert!(
        matches!(ran, Err(ClientError::Failed(ServiceError::Cancelled))),
        "{ran:?}"
    );
    drop((reader, writer));
    let after = within(service_reader.next()).await??;
    assert!(after.is_none(), "a second frame was sent: {after:?}");
    Ok(())
}

#[tokio::test]
async fn refusals_and_lost_streams_are_classified() -> TestResult {
    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    service_writer.reject(ServiceError::VersionMismatch).await?;
    service_writer.flush().await?;
    let refused = within(service::call(
        (&mut reader, &mut writer),
        no_input(),
        std::future::pending(),
    ))
    .await?;
    assert!(matches!(
        refused,
        Err(ClientError::Rejected(ServiceError::VersionMismatch))
    ));

    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    service_writer
        .frame(FrameKind::Accept, &encode_call_accept(b"")?)
        .await?;
    service_writer.shutdown().await?;
    let lost = within(service::call(
        (&mut reader, &mut writer),
        no_input(),
        std::future::pending(),
    ))
    .await?;
    assert!(matches!(
        lost,
        Err(ClientError::Stream(ServiceError::OutcomeUnknown))
    ));
    Ok(())
}

#[tokio::test]
async fn discovery_reads_one_page_or_its_refusal() -> TestResult {
    let ((mut reader, _writer), (_service_reader, mut service_writer)) = pipe();
    let page = DescribePage::new(Vec::new(), Some("next".to_owned()));
    service_writer
        .frame(
            FrameKind::Outcome,
            &DescribePage::encode_outcome(Ok(&page))?,
        )
        .await?;
    service_writer.flush().await?;
    assert_eq!(within(service::describe(&mut reader)).await??, page);

    let ((mut reader, _writer), (_service_reader, mut service_writer)) = pipe();
    service_writer.reject(ServiceError::Unauthorized).await?;
    service_writer.flush().await?;
    assert!(matches!(
        within(service::describe(&mut reader)).await?,
        Err(ClientError::Rejected(ServiceError::Unauthorized))
    ));
    Ok(())
}

/// Reads one `logs` answer of `max` bytes from a target that sends `abc` on stdout and `de`
/// on stderr, returning the read and what reached each sink.
async fn logs_of(max: u32) -> TestResult<(Result<LogsRead, ClientError>, Vec<u8>, Vec<u8>)> {
    let ((mut reader, _writer), (_service_reader, mut service_writer)) = pipe();
    let accept = ProcessAccept::Reading {
        state: ProcessState::Exited,
        start: 0,
        window: LogWindow::new(0, 5),
        truncated: false,
    };
    service_writer
        .frame(FrameKind::Accept, &accept.encode()?)
        .await?;
    service_writer.data(Channel::Output, b"abc").await?;
    service_writer.data(Channel::Error, b"de").await?;
    let reply = ProcessReply::encode_outcome(&Ok(ProcessReply::Logs(5, true)))?;
    service_writer.frame(FrameKind::Outcome, &reply).await?;
    service_writer.flush().await?;
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let read = within(hosted::logs(&mut reader, (&mut output, &mut error), max)).await?;
    Ok((read, output, error))
}

#[tokio::test]
async fn a_logs_answer_longer_than_requested_is_refused_before_its_excess() -> TestResult {
    // Given: a target that sends five bytes of output.
    // When: they are exactly what was asked for.
    let (read, output, error) = logs_of(5).await?;

    // Then: both channels reach their sinks.
    assert_eq!(read.map(|read| (read.next, read.ended))?, (5, true));
    assert_eq!(
        (output.as_slice(), error.as_slice()),
        (&b"abc"[..], &b"de"[..])
    );

    // When: one byte fewer was asked for.
    let (read, output, error) = logs_of(4).await?;

    // Then: the frame past the limit reaches neither sink.
    assert!(
        matches!(read, Err(ClientError::Stream(ServiceError::InvalidInput))),
        "{read:?}"
    );
    assert_eq!(
        (output.as_slice(), error.as_slice()),
        (&b"abc"[..], &b""[..])
    );
    Ok(())
}

/// A pipe that fails on every write.
struct Broken;

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_listing_entry_no_request_can_name_is_refused() -> TestResult {
    let request = FilesRequest::List(RemotePath::parse("docs:/")?, None);
    for (name, valid) in [("a", true), ("..", false), ("a\nb", false)] {
        // Given: a target that lists one entry under `name`.
        let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
        let entry = DirEntry::new(name.to_owned(), FileMetadata::new(FileKind::File, 1));
        let page = FilesResult::encode_outcome(&Ok(FilesResult::Listing(vec![entry], None)))?;
        service_writer.frame(FrameKind::Outcome, &page).await?;
        service_writer.flush().await?;

        // When
        let listed = within(files::run((&mut reader, &mut writer), &request, &[])).await?;

        // Then
        assert_eq!(listed.is_ok(), valid, "{name:?}: {listed:?}");
    }
    Ok(())
}

/// Runs a three-byte write whose body cannot be sent against a target that accepts it and
/// then answers `outcome`.
async fn write_without_body(
    outcome: &FilesOutcome,
) -> TestResult<Result<(FilesResult, Vec<u8>), ClientError>> {
    let mut replies = FrameWriter::new(Vec::new());
    replies.frame(FrameKind::Accept, &EMPTY_ACCEPT).await?;
    replies
        .frame(FrameKind::Outcome, &FilesResult::encode_outcome(outcome)?)
        .await?;
    replies.flush().await?;
    let replies = replies.into_inner();
    let request = FilesRequest::Write(RemotePath::parse("docs:/a")?, WriteMode::Replace, 3);
    let io = (
        &mut FrameReader::new(replies.as_slice()),
        &mut FrameWriter::new(Broken),
    );
    within(files::run(io, &request, b"abc")).await
}

#[tokio::test]
async fn a_write_whose_body_was_not_sent_never_reports_success() -> TestResult {
    // Given: a target that accepts a write and then claims the full size was written, while
    // the body never left the caller.
    let written = write_without_body(&Ok(FilesResult::Written(3))).await?;

    // Then: the failed send decides.
    assert!(
        matches!(written, Err(ClientError::Stream(_))),
        "{written:?}"
    );

    // And: a failure the target reports still explains why.
    let failed = write_without_body(&Err(ServiceError::Io)).await?;
    assert!(
        matches!(failed, Err(ClientError::Failed(ServiceError::Io))),
        "{failed:?}"
    );
    Ok(())
}
