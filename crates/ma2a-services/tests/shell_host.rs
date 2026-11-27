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
use support::{ClientIo, TempRoot, TestResult, drain, pipe_blocked, pipe_with};
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
async fn a_program_that_closes_its_terminal_runs_until_it_exits() -> TestResult {
    // Given: a program that closes its terminal, then keeps running past the exit drain.
    let request = session(
        "sh",
        &["-c", "exec </dev/null >/dev/null 2>&1; sleep 1; exit 7"],
        60_000,
    )?;
    let ((mut reader, _writer), server) = start(request, &CancellationToken::new());
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When
    let frames = within(drain(&mut reader)).await??;

    // Then: its own exit ends the session, not the closed output.
    let result = outcome(&frames)?;
    assert_eq!(
        (result.termination, result.reason),
        (Termination::Exited(7), EndReason::Completed)
    );
    assert_eq!(server.await?.end, OperationEnd::Completed);
    Ok(())
}

#[tokio::test]
async fn a_caller_that_stops_reading_does_not_hold_an_exited_session_open() -> TestResult {
    // Given: a program that writes more than the caller's stream holds and then exits, and a
    // caller that never reads.
    let request = session("sh", &["-c", "head -c 3000 /dev/zero; exit 0"], 60_000)?;
    let (_client, (mut reader, mut writer)) = pipe_with(256);
    let shutdown = CancellationToken::new();
    let server =
        tokio::spawn(
            async move { serve_shell(request, &shutdown, (&mut reader, &mut writer)).await },
        );

    // Then: the session ends soon after the exit instead of waiting for its 60 s lifetime.
    let report = tokio::time::timeout(Duration::from_secs(15), server).await??;
    assert_ne!(report.end, OperationEnd::Failed(ServiceError::TimedOut));
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
async fn a_caller_that_closes_its_stream_cancels_the_session() -> TestResult {
    // Given
    let ((mut reader, mut writer), server) = start(
        session("sleep", &["30"], 60_000)?,
        &CancellationToken::new(),
    );
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When: the caller closes its side before any terminal.
    writer.shutdown().await?;

    // Then
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::Cancelled);
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::Cancelled)
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

/// Open descriptors of this process on the PTY multiplexer, one per terminal end held.
#[cfg(target_os = "linux")]
fn terminal_masters() -> TestResult<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let target = std::fs::read_link(entry?.path()).unwrap_or_default();
        if target.as_os_str() == "/dev/ptmx" {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn input_a_raw_mode_program_left_unread_does_not_keep_its_terminal() -> TestResult {
    // Given: a raw-mode program that reads nothing and exits shortly.
    let script = "stty raw -echo; sleep 0.5; exit 0";
    let ((mut reader, mut writer), server) = start(
        session("sh", &["-c", script], 60_000)?,
        &CancellationToken::new(),
    );
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When: the caller sends more input than the terminal holds.
    let chunk = vec![b'x'; 16_384];
    let sending = tokio::spawn(async move {
        for _ in 0..32 {
            if writer.data(Channel::Input, &chunk).await.is_err() {
                break;
            }
        }
        writer
    });
    let frames = within(drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::Completed);
    server.await?;

    // Then: nothing keeps the session's terminal once it ended.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(terminal_masters()?, 0);
    drop(within(sending).await?);
    Ok(())
}

#[tokio::test]
async fn a_stop_during_backpressured_output_finishes_the_frame_and_sends_outcome() -> TestResult {
    // Cover lifetime timeout and caller cancel while a flooding PTY program keeps the 64 KiB
    // stream backpressured against a slow reader.
    for (lifetime_ms, expected_reason, expected_end) in [
        (
            250,
            EndReason::TimedOut,
            OperationEnd::Failed(ServiceError::TimedOut),
        ),
        (
            60_000,
            EndReason::Cancelled,
            OperationEnd::Failed(ServiceError::Cancelled),
        ),
    ] {
        let shutdown = CancellationToken::new();
        let request = session("yes", &[], lifetime_ms)?;
        let ((mut reader, mut writer), (mut server_reader, mut server_writer)) =
            pipe_with(64 << 10);
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            let report =
                serve_shell(request, &stop, (&mut server_reader, &mut server_writer)).await;
            (
                report,
                server_writer.is_torn(),
                server_writer.terminal_sent(),
            )
        });

        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        let mut received = 0_u64;
        let mut data_frames = 0_usize;
        let mut triggered = false;
        let result = loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), reader.expect()).await??;
            match frame.kind() {
                FrameKind::Data => {
                    let (channel, bytes) = frame.data()?;
                    assert_eq!(channel, Channel::Output);
                    received += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    data_frames += 1;
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    if !triggered && data_frames == 4 && expected_reason == EndReason::Cancelled {
                        triggered = true;
                        writer.frame(FrameKind::Cancel, &[]).await?;
                    }
                }
                FrameKind::Outcome => {
                    break ProcessResult::decode_outcome(frame.payload())?
                        .map_err(|error| format!("{error:?}"))?;
                }
                other => return Err(format!("unexpected frame {other:?}").into()),
            }
        };
        assert_eq!(result.reason, expected_reason, "{expected_reason:?}");
        assert_eq!(result.output_bytes, received, "{expected_reason:?}");
        let (report, torn, terminal_sent) =
            tokio::time::timeout(Duration::from_secs(10), server).await??;
        assert!(!torn, "{expected_reason:?}: writer was torn");
        assert!(terminal_sent, "{expected_reason:?}: terminal not sent");
        assert_eq!(report.end, expected_end, "{expected_reason:?}");
        assert_eq!(report.bytes_out, received, "{expected_reason:?}");
    }
    Ok(())
}

#[tokio::test]
async fn exit_drain_expiry_mid_frame_finishes_the_frame_and_sends_completed_outcome() -> TestResult
{
    let shutdown = CancellationToken::new();
    let request = session("sh", &["-c", "head -c 2048 /dev/zero; exit 0"], 60_000)?;
    let ((mut reader, _writer), (mut server_reader, mut server_writer), blocked) =
        pipe_blocked(256);
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        let report = serve_shell(request, &stop, (&mut server_reader, &mut server_writer)).await;
        (
            report,
            server_writer.is_torn(),
            server_writer.terminal_sent(),
        )
    });

    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    tokio::time::timeout(Duration::from_secs(5), blocked.notified()).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let frames = tokio::time::timeout(Duration::from_secs(5), drain(&mut reader)).await??;
    let received = u64::try_from(
        frames
            .iter()
            .filter(|frame| frame.kind() == FrameKind::Data)
            .map(|frame| frame.data().expect("data").1.len())
            .sum::<usize>(),
    )?;
    assert!(received > 0);
    let result = outcome(&frames)?;
    assert_eq!(
        (result.termination, result.reason, result.output_bytes),
        (Termination::Exited(0), EndReason::Completed, received)
    );
    let (report, torn, terminal_sent) =
        tokio::time::timeout(Duration::from_secs(5), server).await??;
    assert!(!torn, "writer was torn");
    assert!(terminal_sent, "terminal not sent");
    assert_eq!(report.end, OperationEnd::Completed);
    assert_eq!(report.bytes_out, received);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stop_during_backpressured_output_kills_pty_child_and_stops_inbound_at_once() -> TestResult
{
    let root = TempRoot::new("shell-stop-kill")?;
    let pid_file = root.path().join("pid");
    let script = format!("echo $$ > '{}'; exec yes", pid_file.display());
    let shutdown = CancellationToken::new();
    let request = session("sh", &["-c", &script], 60_000)?;
    let ((mut reader, mut writer), (mut server_reader, mut server_writer), blocked) =
        pipe_blocked(256);
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        serve_shell(request, &stop, (&mut server_reader, &mut server_writer)).await
    });

    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(contents) = std::fs::read_to_string(&pid_file)
            && contents.contains('\n')
        {
            break contents.trim().to_owned();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "child never wrote pid"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tokio::time::timeout(Duration::from_secs(5), blocked.notified()).await?;

    shutdown.cancel();

    let kill_deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    while alive(&pid) && tokio::time::Instant::now() < kill_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !alive(&pid),
        "PTY child {pid} survived past stop while output was backpressured"
    );

    writer.data(Channel::Input, b"after-stop\n").await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let frames = tokio::time::timeout(Duration::from_secs(5), drain(&mut reader)).await??;
    assert_eq!(outcome(&frames)?.reason, EndReason::Shutdown);
    let report = tokio::time::timeout(Duration::from_secs(5), server).await??;
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::Unavailable));
    assert_eq!(report.bytes_in, 0);
    Ok(())
}
