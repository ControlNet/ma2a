//! The caller-side hosted-service driver against a scripted service end.
//!
//! The service end here is a test script writing exact Services v1 frames, not a service.

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    Channel, DescribePage, FrameKind, ServiceError, encode_call_accept, encode_call_outcome,
};
use ma2a_services::client::{ClientError, process::ProcessIo, service};
use support::{TestResult, pipe};

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
