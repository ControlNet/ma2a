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
    Channel, DescribePage, EMPTY_ACCEPT, FilesOutcome, FilesRequest, FilesResult, FrameKind,
    RemotePath, ServiceError, WriteMode, encode_call_accept, encode_call_outcome,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{ClientError, files, process::ProcessIo, service},
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
