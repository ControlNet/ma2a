//! One in-process supervisor over a real local socket: two-phase start, output, stdin,
//! signals, termination, the ownership proof, and the go deadline.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::{io, path::PathBuf, time::Duration};

use ma2a_core::service::{
    Channel as DataChannel, FrameKind, LogFrom, ProcessId, ProcessSignal, ProcessSpec,
    ProcessStart, ServiceError, Termination,
};
use ma2a_services::{
    Frame,
    supervisor::{
        Channel, Credentials, EndCause, GO_DEADLINE, Launch, SupervisorAddress, SupervisorRequest,
        SupervisorStatus, launch::ready, log, log_directory, protocol::decode_reading,
        protocol::decode_written, status, supervise,
    },
};
use support::{TempRoot, TestResult};
use tokio::task::JoinHandle;

const MIB: u32 = 1_048_576;

struct Harness {
    root: TempRoot,
    launch: Launch,
    task: JoinHandle<io::Result<()>>,
}

fn start(script: &str, stdin: bool) -> TestResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?;
    Ok(ProcessStart::new(spec, stdin, MIB)?)
}

fn credentials() -> TestResult<Credentials> {
    let mut secret = [0; 32];
    getrandom::fill(&mut secret)?;
    Ok(Credentials::new(ProcessId::random()?, secret))
}

fn host(label: &str, start: ProcessStart) -> TestResult<Harness> {
    let root = TempRoot::new(label)?;
    let address = root.path().join("p.sock");
    let launch = Launch {
        credentials: credentials()?,
        directory: root.path().join("process"),
        address: SupervisorAddress::new(address.to_str().ok_or("path")?.to_owned()),
        start,
    };
    let task = tokio::spawn(supervise(launch.clone()));
    Ok(Harness { root, launch, task })
}

impl Harness {
    async fn open(&self, request: SupervisorRequest) -> TestResult<Channel> {
        let (reader, mut writer) = ready(&self.launch.address, &self.launch.credentials).await?;
        writer.frame(FrameKind::Open, &request.to_bytes()).await?;
        Ok((reader, writer))
    }

    /// Sends one request and returns the status in its `outcome`.
    async fn ask(&self, request: SupervisorRequest) -> TestResult<SupervisorStatus> {
        let (mut reader, _writer) = self.open(request).await?;
        let frame = reader.expect().await?;
        assert_eq!(frame.kind(), FrameKind::Outcome, "{frame:?}");
        Ok(SupervisorStatus::from_bytes(frame.payload())?)
    }

    fn directory(&self) -> PathBuf {
        self.launch.directory.clone()
    }

    async fn finish(self) -> TestResult<Option<SupervisorStatus>> {
        let Self { root, launch, task } = self;
        tokio::time::timeout(GO_DEADLINE + Duration::from_secs(20), task).await???;
        let recorded = status::read(&launch.directory)?;
        let socket_left = std::fs::exists(launch.address.as_str())?;
        assert!(!socket_left, "the socket is removed on exit");
        drop(root);
        Ok(recorded)
    }
}

/// Reads frames until the `outcome`, returning output by channel and the final status.
async fn follow(channel: &mut Channel) -> TestResult<(Vec<u8>, Vec<u8>, Vec<Frame>)> {
    let (mut output, mut error, mut controls) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let frame = channel.0.expect().await?;
        match frame.kind() {
            FrameKind::Data => match frame.data()? {
                (DataChannel::Output, bytes) => output.extend_from_slice(bytes),
                (DataChannel::Error, bytes) => error.extend_from_slice(bytes),
                (DataChannel::Input, _) => return Err("input from the supervisor".into()),
            },
            FrameKind::Control => controls.push(frame),
            FrameKind::Outcome => {
                controls.push(frame);
                return Ok((output, error, controls));
            }
            _ => return Err(format!("unexpected {frame:?}").into()),
        }
    }
}

fn last_status(frames: &[Frame]) -> TestResult<SupervisorStatus> {
    let last = frames.last().ok_or("no outcome")?;
    Ok(SupervisorStatus::from_bytes(last.payload())?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn go_runs_the_child_and_attach_replays_all_output() -> TestResult {
    let harness = host("sup-go", start("echo out; echo err >&2; exit 3", false)?)?;
    let running = harness.ask(SupervisorRequest::Go).await?;
    assert!(
        matches!(running, SupervisorStatus::Running(pid, _) if pid > 0)
            || matches!(running, SupervisorStatus::Ended(..)),
        "{running:?}"
    );
    let mut attached = harness
        .open(SupervisorRequest::Attach(LogFrom::Start, false))
        .await?;
    let accept = attached.0.expect().await?;
    assert_eq!(accept.kind(), FrameKind::Accept);
    let ((from, first, _), truncated) = decode_reading(accept.payload())?;
    assert_eq!((from, first, truncated), (0, 0, false));
    let (output, error, frames) = follow(&mut attached).await?;
    assert_eq!(
        (output.as_slice(), error.as_slice()),
        (&b"out\n"[..], &b"err\n"[..])
    );
    let ended = last_status(&frames)?;
    assert!(
        matches!(
            ended,
            SupervisorStatus::Ended(Termination::Exited(3), EndCause::Exited, _)
        ),
        "{ended:?}"
    );
    // A second go is idempotent and answers with the final status.
    assert_eq!(harness.ask(SupervisorRequest::Go).await?, ended);
    drop(attached);
    assert_eq!(harness.finish().await?, Some(ended));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_writer_relays_stdin_while_a_second_is_refused() -> TestResult {
    let harness = host("sup-stdin", start("cat", true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let mut writer = harness
        .open(SupervisorRequest::Attach(LogFrom::Now, true))
        .await?;
    assert_eq!(writer.0.expect().await?.kind(), FrameKind::Accept);

    let (mut busy, _busy_writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
    let refused = busy.expect().await?;
    assert_eq!(refused.rejection()?, ServiceError::WriterBusy);

    writer.1.data(DataChannel::Input, b"echoed").await?;
    writer.1.end(DataChannel::Input).await?;
    let (output, _, frames) = follow(&mut writer).await?;
    assert_eq!(output, b"echoed");
    let ended = last_status(&frames)?;
    assert!(
        matches!(
            ended,
            SupervisorStatus::Ended(Termination::Exited(0), EndCause::Exited, _)
        ),
        "{ended:?}"
    );
    drop(writer);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_writer_is_refused_at_once_while_the_first_is_blocked() -> TestResult {
    // Given: a writer whose input fills the stdin of a child that never reads it.
    let harness = host("sup-stdin-blocked", start("sleep 30", true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let (mut first, mut first_writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
    assert_eq!(first.expect().await?.kind(), FrameKind::Accept);
    // The input is sent in the background: once the pipe and the socket are full, these
    // writes wait for a child that never reads.
    let filling = tokio::spawn(async move {
        let chunk = vec![b'x'; 65_536];
        for _ in 0..8 {
            if first_writer.data(DataChannel::Input, &chunk).await.is_err() {
                break;
            }
        }
        first_writer
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // When
    let (mut second, _second_writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
    let answer = tokio::time::timeout(Duration::from_secs(2), second.expect()).await??;

    // Then: the lease decides, not a wait behind the blocked write.
    assert_eq!(answer.rejection()?, ServiceError::WriterBusy);
    harness.ask(SupervisorRequest::Terminate(0)).await?;
    drop((first, filling.await?));
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_writes_report_their_count_and_closing() -> TestResult {
    let harness = host("sup-stdin-op", start("cat", true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let mut attached = harness
        .open(SupervisorRequest::Attach(LogFrom::Start, false))
        .await?;
    assert_eq!(attached.0.expect().await?.kind(), FrameKind::Accept);
    let (mut reader, mut writer) = harness.open(SupervisorRequest::Stdin(true)).await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(DataChannel::Input, b"12345").await?;
    writer.end(DataChannel::Input).await?;
    let outcome = reader.expect().await?;
    assert_eq!(decode_written(outcome.payload())?, (5, true));
    // With stdin closed, cat ends by itself.
    let (output, _, _) = follow(&mut attached).await?;
    assert_eq!(output, b"12345");
    drop(attached);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stdin_write_ends_when_the_child_does() -> TestResult {
    // Given: a writer that never ends its input to a child that exits on its own.
    let harness = host("sup-stdin-exit", start("read line; exit 0", true)?)?;
    // A watcher keeps the supervisor reachable after the end, as the Runtime's follower does.
    let mut watch = harness.open(SupervisorRequest::Watch).await?;
    assert_eq!(watch.0.expect().await?.kind(), FrameKind::Accept);
    harness.ask(SupervisorRequest::Go).await?;
    let (mut reader, mut writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);

    // When
    writer.data(DataChannel::Input, b"line\n").await?;

    // Then: the write reports what reached the child, and that its stdin is gone.
    let outcome = tokio::time::timeout(Duration::from_secs(10), reader.expect()).await??;
    assert_eq!(outcome.kind(), FrameKind::Outcome);
    assert_eq!(decode_written(outcome.payload())?, (5, true));
    drop((reader, writer, watch));
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_reaches_the_child_group() -> TestResult {
    let harness = host("sup-signal", start("sleep 60 & wait", false)?)?;
    // A watcher keeps the supervisor reachable until it has read the end.
    let mut watch = harness.open(SupervisorRequest::Watch).await?;
    assert_eq!(watch.0.expect().await?.kind(), FrameKind::Accept);
    harness.ask(SupervisorRequest::Go).await?;
    let (mut reader, _writer) = harness
        .open(SupervisorRequest::Signal(ProcessSignal::User1))
        .await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Outcome);
    let (_, _, frames) = follow(&mut watch).await?;
    let ended = last_status(&frames)?;
    assert!(
        matches!(
            ended,
            SupervisorStatus::Ended(Termination::Signaled(10), EndCause::Exited, _)
        ),
        "{ended:?}"
    );
    drop(watch);
    assert_eq!(harness.finish().await?, Some(ended));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_kills_after_the_grace_period() -> TestResult {
    let harness = host(
        "sup-kill",
        start("trap '' TERM; while :; do sleep 1; done", false)?,
    )?;
    harness.ask(SupervisorRequest::Go).await?;
    let started = std::time::Instant::now();
    let ended = harness.ask(SupervisorRequest::Terminate(300)).await?;
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(
        matches!(
            ended,
            SupervisorStatus::Ended(Termination::Signaled(9), EndCause::Terminated, _)
        ),
        "{ended:?}"
    );
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_go_no_child_exists() -> TestResult {
    let harness = host(
        "sup-abort",
        start("touch /never-created-by-ma2a-tests", false)?,
    )?;
    let (mut reader, _writer) = harness
        .open(SupervisorRequest::Signal(ProcessSignal::Terminate))
        .await?;
    assert_eq!(
        reader.expect().await?.rejection()?,
        ServiceError::NotRunning
    );
    let aborted = harness.ask(SupervisorRequest::Abort).await?;
    assert!(
        matches!(aborted, SupervisorStatus::NeverStarted(_)),
        "{aborted:?}"
    );
    assert_eq!(harness.finish().await?, Some(aborted));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_before_the_child_exists_is_not_running_and_without_stdin_is_closed() -> TestResult {
    let harness = host("sup-stdin-early", start("cat", true)?)?;
    let (mut early, _early_writer) = harness
        .open(SupervisorRequest::Attach(LogFrom::Now, true))
        .await?;
    assert_eq!(early.expect().await?.rejection()?, ServiceError::NotRunning);
    harness.ask(SupervisorRequest::Abort).await?;
    harness.finish().await?;

    let harness = host("sup-stdin-none", start("true", false)?)?;
    let (mut closed, _closed_writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
    assert_eq!(
        closed.expect().await?.rejection()?,
        ServiceError::StdinClosed
    );
    harness.ask(SupervisorRequest::Abort).await?;
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_after_the_child_ended_is_not_running() -> TestResult {
    // Given: a child with stdin that ended while a watcher keeps the supervisor reachable.
    let harness = host("sup-stdin-ended", start("exit 0", true)?)?;
    let mut watch = harness.open(SupervisorRequest::Watch).await?;
    assert_eq!(watch.0.expect().await?.kind(), FrameKind::Accept);
    harness.ask(SupervisorRequest::Go).await?;
    let (_, _, frames) = follow(&mut watch).await?;
    assert!(matches!(last_status(&frames)?, SupervisorStatus::Ended(..)));

    // When / Then: no write or writing attachment is taken any more.
    for request in [
        SupervisorRequest::Stdin(false),
        SupervisorRequest::Attach(LogFrom::Now, true),
    ] {
        let (mut reader, _writer) = harness.open(request).await?;
        let answer = reader.expect().await?;
        assert_eq!(answer.kind(), FrameKind::Reject, "{answer:?}");
        assert_eq!(answer.rejection()?, ServiceError::NotRunning);
    }
    drop(watch);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_of_a_program_that_never_started_is_not_running() -> TestResult {
    // Given: a Process with stdin whose program cannot start.
    let spec = ProcessSpec::new(
        Some("/nonexistent/ma2a-test-program".to_owned()),
        Vec::new(),
        Some("/".to_owned()),
    )?;
    let harness = host("sup-stdin-failed", ProcessStart::new(spec, true, MIB)?)?;
    let mut watch = harness.open(SupervisorRequest::Watch).await?;
    assert_eq!(watch.0.expect().await?.kind(), FrameKind::Accept);
    let failed = harness.ask(SupervisorRequest::Go).await?;
    assert!(
        matches!(failed, SupervisorStatus::SpawnFailed(_)),
        "{failed:?}"
    );

    // When
    let (mut reader, _writer) = harness.open(SupervisorRequest::Stdin(false)).await?;

    // Then: it never ran, so its stdin was never closed either.
    assert_eq!(
        reader.expect().await?.rejection()?,
        ServiceError::NotRunning
    );
    drop(watch);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_that_cannot_start_is_recorded() -> TestResult {
    let spec = ProcessSpec::new(
        Some("/nonexistent/ma2a-test-program".to_owned()),
        Vec::new(),
        Some("/".to_owned()),
    )?;
    let harness = host("sup-spawn-failed", ProcessStart::new(spec, false, MIB)?)?;
    let failed = harness.ask(SupervisorRequest::Go).await?;
    assert!(
        matches!(failed, SupervisorStatus::SpawnFailed(_)),
        "{failed:?}"
    );
    assert_eq!(harness.finish().await?, Some(failed));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_without_the_secret_is_refused_both_ways() -> TestResult {
    let harness = host("sup-proof", start("exit 0", false)?)?;
    ready(&harness.launch.address, &harness.launch.credentials).await?;
    let impostor = Credentials::new(harness.launch.credentials.id(), [7; 32]);
    let refused = harness.launch.address.connect(&impostor).await;
    assert!(
        matches!(refused, Err(ServiceError::Unauthorized)),
        "{refused:?}"
    );
    // A refused peer changed nothing: the child is still waiting for a proven go.
    assert_eq!(status::read(&harness.directory())?, None);
    harness.ask(SupervisorRequest::Abort).await?;
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_over_the_limit_is_truncated_for_late_readers() -> TestResult {
    const TOTAL: u64 = 4_194_304;
    let harness = host(
        "sup-overflow",
        start("head -c 4194304 /dev/zero; exec sleep 60", false)?,
    )?;
    harness.ask(SupervisorRequest::Go).await?;
    let output = log_directory(&harness.directory());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while log::window(&output)?.end < TOTAL {
        assert!(
            tokio::time::Instant::now() < deadline,
            "output never arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut late = harness
        .open(SupervisorRequest::Attach(LogFrom::Cursor(0), false))
        .await?;
    let accept = late.0.expect().await?;
    let ((from, first, end), truncated) = decode_reading(accept.payload())?;
    assert!(truncated);
    assert_eq!((from, end), (first, TOTAL));
    assert!(first > 0);
    let mut received = 0_u64;
    while received < end - first {
        let frame = late.0.expect().await?;
        let (DataChannel::Output, bytes) = frame.data()? else {
            return Err("unexpected channel".into());
        };
        assert!(bytes.iter().all(|byte| *byte == 0));
        received += u64::try_from(bytes.len())?;
    }
    assert_eq!(received, end - first);
    let ended = harness.ask(SupervisorRequest::Terminate(1_000)).await?;
    assert!(
        matches!(ended, SupervisorStatus::Ended(_, EndCause::Terminated, _)),
        "{ended:?}"
    );
    let (rest, _, frames) = follow(&mut late).await?;
    assert!(rest.is_empty());
    assert_eq!(last_status(&frames)?, ended);
    drop(late);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn without_go_the_supervisor_gives_up_and_never_spawns() -> TestResult {
    let marker = std::env::temp_dir().join("ma2a-go-deadline-marker");
    let script = format!("touch {}", marker.display());
    let harness = host("sup-deadline", start(&script, false)?)?;
    let recorded = harness.finish().await?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "{recorded:?}"
    );
    assert!(!std::fs::exists(&marker)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_or_stale_endpoint_is_unreachable_not_slow() -> TestResult {
    let root = TempRoot::new("sup-absent")?;
    let path = root.path().join("gone.sock");
    let address = SupervisorAddress::new(path.to_str().ok_or("path")?.to_owned());
    let credentials = credentials()?;
    let missing = address.connect(&credentials).await;
    assert!(
        matches!(missing, Err(ServiceError::Unreachable)),
        "{missing:?}"
    );
    // A killed supervisor leaves its socket file behind; nobody accepts on it.
    drop(address.bind()?);
    assert!(std::fs::exists(&path)?);
    let stale = address.connect(&credentials).await;
    assert!(matches!(stale, Err(ServiceError::Unreachable)), "{stale:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_lost_or_speaking_no_frames_during_the_proof_is_never_a_cancel() -> TestResult {
    use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
    use tokio::io::AsyncWriteExt as _;
    let root = TempRoot::new("sup-peer")?;
    let path = root.path().join("peer.sock");
    let address = SupervisorAddress::new(path.to_str().ok_or("path")?.to_owned());
    let listener = address.bind()?;
    let credentials = credentials()?;
    for (speaks, expected) in [
        // Gone before it answered: maybe a supervisor that just ended, so tried again later.
        (false, ServiceError::Unavailable),
        // Something that does not speak frames is not this Process's supervisor.
        (true, ServiceError::Unauthorized),
    ] {
        let peer = async {
            let stream = listener.accept().await?;
            if speaks {
                let (_receive, mut send) = stream.split();
                send.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await?;
                send.flush().await?;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            TestResult::Ok(())
        };
        let (connected, accepted) = tokio::join!(address.connect(&credentials), peer);
        accepted?;
        assert!(
            matches!(connected, Err(ref error) if *error == expected),
            "{expected:?}: {connected:?}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipes_a_grandchild_keeps_open_are_drained_once_and_never_logged_after_the_end()
-> TestResult {
    // Given: a child that leaves a grandchild in its own session holding stdout and stderr,
    // which writes to both after the drain would have ended.
    let script = "setsid sh -c 'echo pid:$$; sleep 7; echo late; echo late >&2; sleep 10' & sleep 0.5; exit 0";
    let harness = host("sup-drain", start(script, false)?)?;
    let started = tokio::time::Instant::now();
    harness.ask(SupervisorRequest::Go).await?;

    // When: the child exits.
    let mut attached = harness
        .open(SupervisorRequest::Attach(LogFrom::Start, false))
        .await?;
    assert_eq!(attached.0.expect().await?.kind(), FrameKind::Accept);
    let (output, _, frames) = follow(&mut attached).await?;

    // Then: both pipes share one drain, and nothing is logged after the final status.
    let ended = started.elapsed();
    assert!(
        matches!(last_status(&frames)?, SupervisorStatus::Ended(..)),
        "{frames:?}"
    );
    let pid = String::from_utf8(output)?
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("pid:"))
        .ok_or("no pid")?
        .to_owned();
    tokio::time::sleep_until(started + Duration::from_secs(9)).await;
    let logged = log::read(&log_directory(&harness.directory()), 0, 65_536);
    let _killed = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .status();
    assert!(ended < Duration::from_secs(8), "ended after {ended:?}");
    let late: Vec<_> = logged?
        .records
        .into_iter()
        .filter(|(_, bytes)| bytes.windows(4).any(|window| window == b"late"))
        .collect();
    assert!(late.is_empty(), "{late:?}");
    drop(attached);
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detach_while_output_is_backed_up_still_ends_in_whole_frames_and_a_status() -> TestResult
{
    // Given: an attachment to a busy child whose reader has stopped taking output.
    let harness = host(
        "sup-detach-busy",
        start("head -c 4194304 /dev/zero; exec sleep 60", false)?,
    )?;
    harness.ask(SupervisorRequest::Go).await?;
    let mut attached = harness
        .open(SupervisorRequest::Attach(LogFrom::Start, false))
        .await?;
    assert_eq!(attached.0.expect().await?.kind(), FrameKind::Accept);
    let output = log_directory(&harness.directory());
    while log::window(&output)?.end < 4_194_304 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // When: the reader detaches.
    attached.1.frame(FrameKind::Cancel, &[]).await?;

    // Then: every frame up to the outcome arrives whole, and the outcome is the status.
    let (_, _, frames) = follow(&mut attached).await?;
    assert!(
        matches!(last_status(&frames)?, SupervisorStatus::Running(..)),
        "{frames:?}"
    );
    drop(attached);
    harness.ask(SupervisorRequest::Terminate(0)).await?;
    harness.finish().await?;
    Ok(())
}

/// Whether a new stdin writer is admitted within `wait`, while an earlier one may still be
/// letting go of the lease.
async fn admitted_within(harness: &Harness, wait: Duration) -> TestResult<bool> {
    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        let (mut reader, _writer) = harness.open(SupervisorRequest::Stdin(false)).await?;
        let answer = reader.expect().await?;
        if answer.kind() == FrameKind::Accept {
            return Ok(true);
        }
        assert_eq!(answer.rejection()?, ServiceError::WriterBusy);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(false)
}

/// Sends two pipes' worth of stdin, so the second write waits for a child that never reads.
async fn fill_stdin(channel: &mut Channel) -> TestResult {
    let chunk = vec![b'x'; 65_536];
    for _ in 0..2 {
        channel.1.data(DataChannel::Input, &chunk).await?;
    }
    channel.1.flush().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stdin_writer_that_goes_away_during_a_write_frees_the_lease() -> TestResult {
    // Given: a write that waits for a child that never reads its stdin, with the input ended.
    let harness = host("sup-stdin-gone", start("sleep 30", true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let mut first = harness.open(SupervisorRequest::Stdin(false)).await?;
    assert_eq!(first.0.expect().await?.kind(), FrameKind::Accept);
    fill_stdin(&mut first).await?;
    first.1.end(DataChannel::Input).await?;
    first.1.flush().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // When: the daemon gives up on the write and closes its connection.
    drop(first);

    // Then: the next writer is admitted.
    assert!(admitted_within(&harness, Duration::from_secs(3)).await?);
    harness.ask(SupervisorRequest::Terminate(0)).await?;
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writing_attachment_detaches_during_a_write_and_frees_the_lease() -> TestResult {
    // Given: an attachment whose input waits for a child that never reads its stdin.
    let harness = host("sup-attach-gone", start("sleep 30", true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let mut attached = harness
        .open(SupervisorRequest::Attach(LogFrom::Now, true))
        .await?;
    assert_eq!(attached.0.expect().await?.kind(), FrameKind::Accept);
    fill_stdin(&mut attached).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // When
    attached.1.frame(FrameKind::Cancel, &[]).await?;
    attached.1.flush().await?;

    // Then: the detach is answered with the status, and the next writer is admitted.
    let (_, _, frames) =
        tokio::time::timeout(Duration::from_secs(3), follow(&mut attached)).await??;
    assert!(
        matches!(last_status(&frames)?, SupervisorStatus::Running(..)),
        "{frames:?}"
    );
    assert!(admitted_within(&harness, Duration::from_secs(3)).await?);
    drop(attached);
    harness.ask(SupervisorRequest::Terminate(0)).await?;
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stdin_write_waiting_on_a_pipe_a_grandchild_keeps_ends_with_the_child() -> TestResult {
    // Given: a child that leaves a grandchild in its own session holding its stdin without
    // reading it.
    // A background job's stdin is /dev/null, so the pipe is handed over on another descriptor.
    let script = "exec 3<&0; setsid sleep 5 <&3 >/dev/null 2>&1 & exec 3<&-; sleep 0.5; exit 0";
    let harness = host("sup-stdin-grandchild", start(script, true)?)?;
    let mut watch = harness.open(SupervisorRequest::Watch).await?;
    assert_eq!(watch.0.expect().await?.kind(), FrameKind::Accept);
    harness.ask(SupervisorRequest::Go).await?;
    let mut writing = harness.open(SupervisorRequest::Stdin(false)).await?;
    assert_eq!(writing.0.expect().await?.kind(), FrameKind::Accept);

    // When: a write waits on that pipe while the child exits.
    fill_stdin(&mut writing).await?;

    // Then: the write reports what reached the pipe, and that stdin is gone, at the child's end.
    let outcome = tokio::time::timeout(Duration::from_secs(3), writing.0.expect()).await??;
    assert_eq!(outcome.kind(), FrameKind::Outcome, "{outcome:?}");
    assert_eq!(decode_written(outcome.payload())?, (65_536, true));
    drop((writing, watch));
    harness.finish().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_attach_detaches_on_closed_stdin_while_lenient_attach_keeps_reading() -> TestResult {
    // Given: a running child that closes stdin and keeps writing output.
    let script = "exec <&-; echo ready; sleep 0.2; echo still-alive; exec sleep 60";
    let harness = host("sup-attach-strict-stdin", start(script, true)?)?;
    harness.ask(SupervisorRequest::Go).await?;
    let output = log_directory(&harness.directory());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while log::window(&output)?.end < 6 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "child never wrote ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // When: an operator Attach(true) writes to the closed stdin, then cancels after reading output.
    let mut lenient = harness
        .open(SupervisorRequest::Attach(LogFrom::Start, true))
        .await?;
    assert_eq!(lenient.0.expect().await?.kind(), FrameKind::Accept);
    lenient.1.data(DataChannel::Input, b"ignored\n").await?;
    lenient.1.flush().await?;
    let frame = tokio::time::timeout(Duration::from_secs(3), lenient.0.expect()).await??;
    assert_eq!(frame.kind(), FrameKind::Data, "{frame:?}");
    lenient.1.frame(FrameKind::Cancel, &[]).await?;
    lenient.1.flush().await?;
    let (_, _, frames) = follow(&mut lenient).await?;
    assert!(matches!(
        last_status(&frames)?,
        SupervisorStatus::Running(..)
    ));
    drop(lenient);

    // Then: a fresh Attach(true) is now rejected as StdinClosed, and even before any prior write
    // a strict attachment detaches on write failure and rejects the next AttachStrict.
    let (mut rejected, _writer) = harness
        .open(SupervisorRequest::Attach(LogFrom::Now, true))
        .await?;
    assert_eq!(
        rejected.expect().await?.rejection()?,
        ServiceError::StdinClosed
    );

    let harness_strict = host("sup-attach-strict-only", start(script, true)?)?;
    harness_strict.ask(SupervisorRequest::Go).await?;
    let strict_log = log_directory(&harness_strict.directory());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while log::window(&strict_log)?.end < 6 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "strict child never wrote ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut strict = harness_strict
        .open(SupervisorRequest::AttachStrict(LogFrom::Start))
        .await?;
    assert_eq!(strict.0.expect().await?.kind(), FrameKind::Accept);
    strict.1.data(DataChannel::Input, b"fail-me\n").await?;
    strict.1.flush().await?;
    let (_, _, strict_frames) =
        tokio::time::timeout(Duration::from_secs(3), follow(&mut strict)).await??;
    assert!(matches!(
        last_status(&strict_frames)?,
        SupervisorStatus::Running(..)
    ));
    drop(strict);
    let (mut strict_rejected, _writer) = harness_strict
        .open(SupervisorRequest::AttachStrict(LogFrom::Now))
        .await?;
    assert_eq!(
        strict_rejected.expect().await?.rejection()?,
        ServiceError::StdinClosed
    );

    harness.ask(SupervisorRequest::Terminate(0)).await?;
    harness.finish().await?;
    harness_strict.ask(SupervisorRequest::Terminate(0)).await?;
    harness_strict.finish().await?;
    Ok(())
}
