//! Shell host sessions on a real PTY: I/O, resize, lifetime, cancellation, and shutdown.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    Channel, EndReason, FrameKind, ProcessResult, ProcessSpec, ServiceError, ShellRequest,
    TerminalSize, Termination,
};
use ma2a_services::{Frame, OperationEnd, OperationReport, serve_shell};
use support::{ClientIo, TestResult, drain, pipe_with};
use tokio_util::sync::CancellationToken;

fn session(program: &str, args: &[&str], lifetime_ms: u64) -> TestResult<ShellRequest> {
    let spec = ProcessSpec::new(
        Some(program.to_owned()),
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        Some("/".to_owned()),
    )?;
    Ok(ShellRequest::new(
        spec,
        ("dumb".to_owned(), TerminalSize::new(80, 24)?),
        lifetime_ms,
    )?)
}

fn start(
    request: ShellRequest,
    shutdown: &CancellationToken,
) -> (ClientIo, tokio::task::JoinHandle<OperationReport>) {
    let (client, (mut reader, mut writer)) = pipe_with(1 << 20);
    let shutdown = shutdown.clone();
    let server =
        tokio::spawn(
            async move { serve_shell(request, &shutdown, (&mut reader, &mut writer)).await },
        );
    (client, server)
}

fn output(frames: &[Frame]) -> TestResult<String> {
    let mut bytes = Vec::new();
    for frame in frames
        .iter()
        .filter(|frame| frame.kind() == FrameKind::Data)
    {
        let (channel, chunk) = frame.data()?;
        assert_eq!(channel, Channel::Output);
        bytes.extend_from_slice(chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn outcome(frames: &[Frame]) -> TestResult<ProcessResult> {
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    ProcessResult::decode_outcome(last.payload())?.map_err(|error| format!("{error:?}").into())
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> TestResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(20), future).await?)
}

#[tokio::test]
async fn an_interactive_session_echoes_input_and_reports_its_exit() -> TestResult {
    let ((mut reader, mut writer), server) =
        start(session("sh", &[], 60_000)?, &CancellationToken::new());
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer
        .data(Channel::Input, b"echo ma2a-$((6*7))\nexit 5\n")
        .await?;
    let frames = within(drain(&mut reader)).await??;
    assert!(output(&frames)?.contains("ma2a-42"));
    let result = outcome(&frames)?;
    assert_eq!(
        (result.termination, result.reason),
        (Termination::Exited(5), EndReason::Completed)
    );
    let report = server.await?;
    assert_eq!(report.end, OperationEnd::Completed);
    assert!(report.bytes_in > 0 && report.bytes_out > 0);
    Ok(())
}

#[tokio::test]
async fn resize_reaches_the_terminal() -> TestResult {
    let request = session("sh", &["-c", "sleep 0.5; stty size"], 60_000)?;
    let ((mut reader, mut writer), server) = start(request, &CancellationToken::new());
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let resize = TerminalSize::new(100, 40)?.encode_resize()?;
    writer.frame(FrameKind::Control, &resize).await?;
    let frames = within(drain(&mut reader)).await??;
    assert!(output(&frames)?.contains("40 100"), "{}", output(&frames)?);
    assert_eq!(server.await?.end, OperationEnd::Completed);
    Ok(())
}

#[tokio::test]
async fn lifetime_cancel_and_shutdown_end_the_session() -> TestResult {
    let ((mut reader, _writer), server) =
        start(session("sleep", &["30"], 300)?, &CancellationToken::new());
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::TimedOut);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::TimedOut)
    );

    let ((mut reader, mut writer), server) = start(
        session("sleep", &["30"], 60_000)?,
        &CancellationToken::new(),
    );
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.frame(FrameKind::Cancel, &[]).await?;
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
    );

    let shutdown = CancellationToken::new();
    let ((mut reader, _writer), server) = start(session("sleep", &["30"], 60_000)?, &shutdown);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    shutdown.cancel();
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::Shutdown);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Unavailable)
    );
    Ok(())
}

#[tokio::test]
async fn an_unstartable_program_is_rejected() -> TestResult {
    let ((mut reader, _writer), server) = start(
        session("/nonexistent/ma2a-shell", &[], 1_000)?,
        &CancellationToken::new(),
    );
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(
        frames.first().ok_or("reject")?.rejection()?,
        ServiceError::SpawnFailed
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Rejected(ServiceError::SpawnFailed)
    );
    Ok(())
}

/// Returns whether `pid` still names a live (not yet reaped, not zombie) process.
#[cfg(target_os = "linux")]
fn alive(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z")
    })
}

/// Job control moves `&` jobs into their own process groups, beyond a group kill.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn background_jobs_never_outlive_their_session() -> TestResult {
    for (script, cancel) in [
        ("set -m; sleep 300 & echo job=$!; exit 0", false),
        ("set -m; sleep 300 & echo job=$!; wait", true),
    ] {
        let ((mut reader, mut writer), server) = start(
            session("sh", &["-c", script], 60_000)?,
            &CancellationToken::new(),
        );
        let mut frames = Vec::new();
        let pid = loop {
            let frame = within(reader.expect()).await??;
            frames.push(frame);
            let shown = output(&frames)?;
            if let Some(pid) = shown
                .split("job=")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .filter(|_| shown.contains('\n'))
            {
                break pid.to_owned();
            }
        };
        if cancel {
            writer.frame(FrameKind::Cancel, &[]).await?;
        }
        within(drain(&mut reader)).await??;
        within(server).await??;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while alive(&pid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!alive(&pid), "job {pid} outlived its session ({script})");
    }
    Ok(())
}
