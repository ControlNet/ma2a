//! The caller-side Process driver of `ma2a-services` against this Runtime's real host.

use std::time::Duration;

use ma2a_core::service::{
    EndReason, LogFrom, MAX_LOGS_READ, ProcessAccept, ProcessId, ProcessReply, ProcessRequest,
    ProcessSpec, ProcessStart, ProcessState, ServiceError,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{ClientError, hosted, process::ProcessIo},
};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

use super::{Harness, MIB, TestResult, reaches, request_id};
use crate::processes::{ProcessManager, serve};

type End = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

fn pipe() -> (End, End) {
    let (caller, host) = tokio::io::duplex(1 << 16);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (host_read, host_write) = tokio::io::split(host);
    (
        (
            FrameReader::new(caller_read),
            FrameWriter::new(caller_write),
        ),
        (FrameReader::new(host_read), FrameWriter::new(host_write)),
    )
}

fn stdin_start(script: &str) -> TestResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?;
    Ok(ProcessStart::new(spec, true, MIB)?)
}

/// Runs one operation without a stream body as `harness.creator`.
async fn call(
    (harness, manager): (&Harness, &ProcessManager),
    (byte, request): (u8, ProcessRequest),
) -> TestResult<Result<(Option<ProcessAccept>, ProcessReply), ClientError>> {
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let scope = ((harness.creator, request_id(byte)?), request, &stop);
    let host = serve(manager, scope, (&mut host_reader, &mut host_writer));
    let (_report, answer) = tokio::join!(host, hosted::call(&mut reader));
    Ok(answer)
}

async fn start_cat(harness: &Harness, manager: &ProcessManager, byte: u8) -> TestResult<ProcessId> {
    let request = ProcessRequest::Start(stdin_start("cat")?);
    let (accept, reply) = call((harness, manager), (byte, request)).await??;
    let Some(ProcessAccept::Started(id, false)) = accept else {
        return Err(format!("expected a new Process, got {accept:?}").into());
    };
    assert!(matches!(reply, ProcessReply::Summary(summary) if summary.id == id));
    Ok(id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_retry_stdin_and_logs_run_through_the_driver() -> TestResult {
    let harness = Harness::new("driver-stdin").await?;
    let (manager, shutdown) = harness.manager();
    let id = start_cat(&harness, &manager, 20).await?;
    let retry = ProcessRequest::Start(stdin_start("cat")?);
    let (accept, _) = call((&harness, &manager), (20, retry)).await??;
    assert_eq!(accept, Some(ProcessAccept::Started(id, true)));

    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let scope = (
        (harness.creator, request_id(21)?),
        ProcessRequest::Stdin(id, true),
        &stop,
    );
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let caller = hosted::stdin((&mut reader, &mut writer), &b"hello\n"[..]);
    let (_report, written) = tokio::join!(host, caller);
    assert_eq!(written?, (6, true));
    reaches(&manager, id, ProcessState::Exited).await?;

    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let request = ProcessRequest::Logs(id, LogFrom::Start, MAX_LOGS_READ);
    let scope = ((harness.creator, request_id(22)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let caller = hosted::logs(&mut reader, (&mut output, &mut error));
    let (_report, read) = tokio::join!(host, caller);
    let read = read?;
    assert_eq!(
        (output.as_slice(), read.next, read.ended),
        (&b"hello\n"[..], 6, true)
    );
    assert_eq!(read.gaps.count, 0);
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attach_forwards_stdin_and_a_detach_leaves_the_process_running() -> TestResult {
    let harness = Harness::new("driver-attach").await?;
    let (manager, shutdown) = harness.manager();
    let id = start_cat(&harness, &manager, 30).await?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Start, true);
    let scope = ((harness.creator, request_id(31)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let local = ProcessIo {
        input: Some(&b"ping\n"[..]),
        output: &mut output,
        error: Some(&mut error),
    };
    let caller = hosted::attach((&mut reader, &mut writer), local, std::future::pending());
    let (_report, attached) = tokio::join!(host, caller);
    let attached = attached?;
    assert_eq!(attached.reason, EndReason::Completed);
    assert_eq!((output.as_slice(), attached.cursor), (&b"ping\n"[..], 5));

    let request = ProcessRequest::Start(stdin_start("sleep 30")?);
    let (accept, _) = call((&harness, &manager), (32, request)).await??;
    let Some(ProcessAccept::Started(sleeper, false)) = accept else {
        return Err("expected a new Process".into());
    };
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
    let request = ProcessRequest::Attach(sleeper, LogFrom::Now, false);
    let scope = ((harness.creator, request_id(33)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let local = ProcessIo::<&[u8], _, Vec<u8>> {
        input: None,
        output: Vec::new(),
        error: None,
    };
    let detach = tokio::time::sleep(Duration::from_millis(300));
    let caller = hosted::attach((&mut reader, &mut writer), local, detach);
    let (_report, attached) = tokio::join!(host, caller);
    let attached = attached?;
    assert_eq!(attached.reason, EndReason::Cancelled);
    assert_eq!(attached.summary.state, ProcessState::Running);

    let (_, listed) = call((&harness, &manager), (34, ProcessRequest::List(None))).await??;
    let ProcessReply::Page(page, None) = listed else {
        return Err("expected one page".into());
    };
    assert_eq!(page.len(), 2);
    let (_, ended) = call(
        (&harness, &manager),
        (35, ProcessRequest::Terminate(sleeper, 500)),
    )
    .await??;
    assert!(
        matches!(ended, ProcessReply::Summary(summary) if summary.state == ProcessState::Terminated)
    );
    let (_, deleted) = call((&harness, &manager), (36, ProcessRequest::Delete(sleeper))).await??;
    assert_eq!(deleted, ProcessReply::Deleted);
    let signal = ProcessRequest::Signal(sleeper, ma2a_core::service::ProcessSignal::Interrupt);
    let refused = call((&harness, &manager), (37, signal)).await?;
    assert!(
        matches!(
            refused,
            Err(ClientError::Rejected(ServiceError::NotRunning))
        ),
        "{refused:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_stdin_archived_logs_relative_cwd_and_strangers_are_refused_exactly() -> TestResult {
    let harness = Harness::new("driver-refusals").await?;
    let (manager, shutdown) = harness.manager();
    let relative = ProcessSpec::new(
        Some("/bin/true".to_owned()),
        Vec::new(),
        Some("tmp".to_owned()),
    )?;
    let relative = ProcessRequest::Start(ProcessStart::new(relative, false, MIB)?);
    let refused = call((&harness, &manager), (40, relative)).await?;
    assert!(
        matches!(
            refused,
            Err(ClientError::Rejected(ServiceError::InvalidInput))
        ),
        "{refused:?}"
    );
    let (_, listed) = call((&harness, &manager), (41, ProcessRequest::List(None))).await??;
    assert_eq!(
        listed,
        ProcessReply::Page(Vec::new(), None),
        "nothing was recorded"
    );

    let id = start_cat(&harness, &manager, 42).await?;
    // A stranger learns nothing, not even from the reader limit.
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let stranger = iroh::SecretKey::generate().public().into();
    let scope = (
        (stranger, request_id(43)?),
        ProcessRequest::Attach(id, LogFrom::Now, false),
        &stop,
    );
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let local = ProcessIo::<&[u8], Vec<u8>, Vec<u8>> {
        input: None,
        output: Vec::new(),
        error: None,
    };
    let caller = hosted::attach((&mut reader, &mut writer), local, std::future::pending());
    let (_report, attached) = tokio::join!(host, caller);
    assert!(
        matches!(
            attached,
            Err(ClientError::Rejected(ServiceError::UnknownProcess))
        ),
        "{attached:?}"
    );

    for (byte, expected) in [(44, Ok((1, true))), (45, Err(ServiceError::StdinClosed))] {
        let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe();
        let scope = (
            (harness.creator, request_id(byte)?),
            ProcessRequest::Stdin(id, true),
            &stop,
        );
        let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
        let caller = hosted::stdin((&mut reader, &mut writer), &b"x"[..]);
        let (_report, written) = tokio::join!(host, caller);
        match (written, expected) {
            (Ok(written), Ok(expected)) => assert_eq!(written, expected),
            (Err(ClientError::Rejected(error)), Err(expected)) => assert_eq!(error, expected),
            (written, expected) => {
                return Err(format!("{written:?} instead of {expected:?}").into());
            }
        }
    }
    reaches(&manager, id, ProcessState::Exited).await?;
    let (_, deleted) = call((&harness, &manager), (46, ProcessRequest::Delete(id))).await??;
    assert_eq!(deleted, ProcessReply::Deleted);
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let request = ProcessRequest::Logs(id, LogFrom::Start, MAX_LOGS_READ);
    let scope = ((harness.creator, request_id(47)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let (_report, read) = tokio::join!(host, hosted::logs(&mut reader, (&mut output, &mut error)));
    let read = read?;
    assert_eq!(
        (output.len(), read.next, read.ended),
        (0, 0, true),
        "an archive has no output"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_gone_before_the_accept_still_gets_its_process() -> TestResult {
    let harness = Harness::new("driver-gone").await?;
    let (manager, shutdown) = harness.manager();
    let (caller, (mut host_reader, mut host_writer)) = pipe();
    // The caller disappears right after its `open`: every host write fails.
    drop(caller);
    let stop = CancellationToken::new();
    let request = ProcessRequest::Start(stdin_start("cat")?);
    let scope = ((harness.creator, request_id(23)?), request, &stop);
    let _report = serve(&manager, scope, (&mut host_reader, &mut host_writer)).await;

    // The retry answers the Process the lost attempt started, and it runs.
    let retry = ProcessRequest::Start(stdin_start("cat")?);
    let (accept, _) = call((&harness, &manager), (23, retry)).await??;
    let Some(ProcessAccept::Started(id, true)) = accept else {
        return Err(format!("expected the existing Process, got {accept:?}").into());
    };
    reaches(&manager, id, ProcessState::Running).await?;
    let ended = manager
        .terminate(
            (None, id),
            (1_000, ma2a_core::service::ProcessReason::Requested),
        )
        .await?;
    assert_eq!(ended.state, ProcessState::Terminated);
    shutdown.cancel();
    Ok(())
}
