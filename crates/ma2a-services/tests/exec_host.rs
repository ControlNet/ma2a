//! Exec host lifecycle: streaming, stdin, timeout, cancellation, shutdown, and group kill.
//!
//! Test fixtures run `sh -c` scripts as ordinary typed argv; the host never invokes a shell.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    Channel, EndReason, ExecRequest, FrameKind, MAX_DATA_CHUNK_LEN, ProcessResult, ProcessSpec,
    ServiceError, Termination,
};
use ma2a_services::{Frame, OperationEnd, OperationReport, serve_exec};
use support::{ClientIo, TestResult, drain, pipe_with};
use tokio_util::sync::CancellationToken;

fn script(script: &str) -> TestResult<ProcessSpec> {
    Ok(ProcessSpec::new(
        Some("sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?)
}

fn start(
    request: ExecRequest,
    shutdown: &CancellationToken,
    capacity: usize,
) -> (ClientIo, tokio::task::JoinHandle<OperationReport>) {
    let (client, (mut reader, mut writer)) = pipe_with(capacity);
    let shutdown = shutdown.clone();
    let server =
        tokio::spawn(
            async move { serve_exec(request, &shutdown, (&mut reader, &mut writer)).await },
        );
    (client, server)
}

fn process_outcome(frames: &[Frame]) -> TestResult<ProcessResult> {
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    ProcessResult::decode_outcome(last.payload())?.map_err(|error| format!("{error:?}").into())
}

fn channel_bytes(frames: &[Frame], wanted: Channel) -> TestResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for frame in frames
        .iter()
        .filter(|frame| frame.kind() == FrameKind::Data)
    {
        let (channel, chunk) = frame.data()?;
        assert!(chunk.len() <= MAX_DATA_CHUNK_LEN);
        if channel == wanted {
            bytes.extend_from_slice(chunk);
        }
    }
    Ok(bytes)
}

/// Returns whether a process is gone or only a zombie awaiting its new parent.
fn is_dead(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z'))
    })
}

#[tokio::test]
async fn output_channels_and_exit_code_are_reported() -> TestResult {
    let request = ExecRequest::new(script("printf out; printf err >&2; exit 3")?, false, 10_000)?;
    let ((mut reader, _writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    let frames = drain(&mut reader).await?;
    assert_eq!(frames.first().map(Frame::kind), Some(FrameKind::Accept));
    assert_eq!(channel_bytes(&frames, Channel::Output)?, b"out");
    assert_eq!(channel_bytes(&frames, Channel::Error)?, b"err");
    let result = process_outcome(&frames)?;
    assert_eq!(
        (
            result.termination,
            result.reason,
            result.output_bytes,
            result.error_bytes
        ),
        (Termination::Exited(3), EndReason::Completed, 3, 3)
    );
    assert_eq!(server.await?.end, OperationEnd::Completed);
    Ok(())
}

#[tokio::test]
async fn stdin_is_forwarded_until_end() -> TestResult {
    let spec = ProcessSpec::new(Some("cat".to_owned()), Vec::new(), None)?;
    let ((mut reader, mut writer), server) = start(
        ExecRequest::new(spec, true, 10_000)?,
        &CancellationToken::new(),
        1 << 20,
    );
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"abc").await?;
    writer.data(Channel::Input, b"def").await?;
    writer.end(Channel::Input).await?;
    let frames = drain(&mut reader).await?;
    assert_eq!(channel_bytes(&frames, Channel::Output)?, b"abcdef");
    assert_eq!(
        process_outcome(&frames)?.termination,
        Termination::Exited(0)
    );
    assert_eq!(server.await?.bytes_in, 6);
    Ok(())
}

#[tokio::test]
async fn input_after_the_child_closes_its_stdin_is_dropped() -> TestResult {
    // Given: a child that closes its stdin at once and exits on its own a little later.
    let request = ExecRequest::new(script("exec 0<&-; sleep 0.2; echo ok")?, true, 10_000)?;
    let ((mut reader, mut writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When: the caller keeps sending input it cannot know is unwanted.
    let chunk = vec![b'x'; MAX_DATA_CHUNK_LEN];
    for _ in 0..4 {
        writer.data(Channel::Input, &chunk).await?;
    }
    writer.end(Channel::Input).await?;

    // Then: the run is not torn down; the child finishes and reports its own exit.
    let frames = drain(&mut reader).await?;
    assert_eq!(channel_bytes(&frames, Channel::Output)?, b"ok\n");
    let result = process_outcome(&frames)?;
    assert_eq!(
        (result.termination, result.reason),
        (Termination::Exited(0), EndReason::Completed)
    );
    assert_eq!(server.await?.end, OperationEnd::Completed);
    Ok(())
}

#[tokio::test]
async fn a_timeout_kills_the_child() -> TestResult {
    let started = std::time::Instant::now();
    let request = ExecRequest::new(script("sleep 30")?, false, 200)?;
    let ((mut reader, _writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    let frames = drain(&mut reader).await?;
    let result = process_outcome(&frames)?;
    assert_eq!(
        (result.termination, result.reason),
        (Termination::Signaled(9), EndReason::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::TimedOut)
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_kills_the_whole_process_group() -> TestResult {
    let request = ExecRequest::new(script("sleep 30 & echo $!; wait")?, false, 60_000)?;
    let ((mut reader, mut writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let printed = reader.expect().await?;
    let (_channel, pid) = printed.data()?;
    let pid = String::from_utf8(pid.to_vec())?.trim().to_owned();
    assert!(!is_dead(&pid));
    writer.frame(FrameKind::Cancel, &[]).await?;
    let frames = drain(&mut reader).await?;
    assert_eq!(process_outcome(&frames)?.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(is_dead(&pid), "grandchild {pid} survived cancellation");
    Ok(())
}

#[tokio::test]
async fn a_cancel_behind_input_the_child_never_reads_ends_the_run_at_once() -> TestResult {
    // Given: a child that keeps its stdin open and never reads it.
    let request = ExecRequest::new(script("sleep 30")?, true, 60_000)?;
    let ((mut reader, mut writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When: input fills the 64 KiB pipe and leaves one write waiting, and the caller cancels.
    let chunk = vec![b'x'; 32_768];
    for _ in 0..3 {
        writer.data(Channel::Input, &chunk).await?;
    }
    writer.frame(FrameKind::Cancel, &[]).await?;

    // Then: the run ends as cancelled without waiting for the child.
    let frames = tokio::time::timeout(Duration::from_secs(2), drain(&mut reader)).await??;
    assert_eq!(process_outcome(&frames)?.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    Ok(())
}

#[tokio::test]
async fn a_caller_that_closes_its_stream_before_its_end_of_input_cancels_the_run() -> TestResult {
    // Given: a child that copies its stdin and ends when that input ends.
    let request = ExecRequest::new(script("cat")?, true, 60_000)?;
    let ((mut reader, mut writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(Channel::Input, b"part").await?;

    // When: the caller closes its side without ending its input.
    writer.shutdown().await?;

    // Then: the input is not taken as complete; the run is cancelled.
    let frames = tokio::time::timeout(Duration::from_secs(2), drain(&mut reader)).await??;
    assert_eq!(process_outcome(&frames)?.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );
    Ok(())
}

#[tokio::test]
async fn a_background_child_does_not_outlive_its_exec() -> TestResult {
    let request = ExecRequest::new(script("sleep 30 >/dev/null 2>&1 & echo $!")?, false, 60_000)?;
    let ((mut reader, _writer), server) = start(request, &CancellationToken::new(), 1 << 20);
    let frames = drain(&mut reader).await?;
    let pid = String::from_utf8(channel_bytes(&frames, Channel::Output)?)?
        .trim()
        .to_owned();
    assert_eq!(
        process_outcome(&frames)?.termination,
        Termination::Exited(0)
    );
    assert_eq!(server.await?.end, OperationEnd::Completed);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(is_dead(&pid), "background child {pid} detached");
    Ok(())
}

#[tokio::test]
async fn runtime_shutdown_stops_a_running_exec() -> TestResult {
    let shutdown = CancellationToken::new();
    let request = ExecRequest::new(script("sleep 30")?, false, 60_000)?;
    let ((mut reader, _writer), server) = start(request, &shutdown, 1 << 20);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    shutdown.cancel();
    let frames = drain(&mut reader).await?;
    assert_eq!(process_outcome(&frames)?.reason, EndReason::Shutdown);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Unavailable)
    );
    Ok(())
}

#[tokio::test]
async fn large_output_streams_in_bounded_frames_under_backpressure() -> TestResult {
    let request = ExecRequest::new(script("head -c 5000000 /dev/zero")?, false, 60_000)?;
    let ((mut reader, _writer), server) = start(request, &CancellationToken::new(), 64 << 10);
    let mut total = 0_usize;
    let mut last = None;
    while let Some(frame) = reader.next().await? {
        if frame.kind() == FrameKind::Data {
            total += frame.data()?.1.len();
            tokio::task::yield_now().await;
        }
        last = Some(frame);
    }
    assert_eq!(total, 5_000_000);
    let frames = last.into_iter().collect::<Vec<_>>();
    assert_eq!(process_outcome(&frames)?.output_bytes, 5_000_000);
    assert_eq!(server.await?.bytes_out, 5_000_000);
    Ok(())
}

#[tokio::test]
async fn unstartable_programs_are_rejected_before_any_effect() -> TestResult {
    for (spec, expected) in [
        (
            ProcessSpec::new(
                Some("/nonexistent/ma2a-program".to_owned()),
                Vec::new(),
                None,
            )?,
            ServiceError::SpawnFailed,
        ),
        (
            ProcessSpec::new(
                Some("true".to_owned()),
                Vec::new(),
                Some("relative".to_owned()),
            )?,
            ServiceError::InvalidInput,
        ),
    ] {
        let request = ExecRequest::new(spec, false, 1_000)?;
        let ((mut reader, _writer), server) = start(request, &CancellationToken::new(), 1 << 20);
        let frames = drain(&mut reader).await?;
        assert_eq!(frames.first().ok_or("reject")?.rejection()?, expected);
        assert_eq!(server.await?.end, OperationEnd::Rejected(expected));
    }
    Ok(())
}

#[tokio::test]
async fn an_accept_nobody_reads_ends_the_exec_on_shutdown() -> TestResult {
    // Given: a caller that never reads, so not even `accept` fits in its stream.
    let shutdown = CancellationToken::new();
    let request = ExecRequest::new(script("sleep 30")?, false, 60_000)?;
    let ((_reader, _writer), server) = start(request, &shutdown, 1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // When
    shutdown.cancel();

    // Then: the host stops waiting for the write and kills the child.
    let report = tokio::time::timeout(Duration::from_secs(5), server).await??;
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::Unavailable));
    Ok(())
}

#[tokio::test]
async fn a_caller_that_stops_reading_does_not_hold_an_exited_run_open() -> TestResult {
    // Given: a child that writes more than the caller's stream holds and then exits, and a
    // caller that never reads.
    let request = ExecRequest::new(script("head -c 100000 /dev/zero; exit 0")?, false, 60_000)?;
    let ((_reader, _writer), server) = start(request, &CancellationToken::new(), 1024);

    // Then: the run ends soon after the exit instead of waiting for its 60 s lifetime.
    let report = tokio::time::timeout(Duration::from_secs(15), server).await??;
    assert_ne!(report.end, OperationEnd::Failed(ServiceError::TimedOut));
    Ok(())
}
