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
