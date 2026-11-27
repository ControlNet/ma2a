//! Exec and Shell between real daemons: typed argv, status, streams, terminal I/O, resize.

use std::time::Duration;

use ma2a_core::service::{OpenBody, ProcessSpec, ShellRequest, TerminalSize, Termination};
use ma2a_runtime::{
    api,
    ipc::{IpcPaths, LocalApiClient, StreamUpgrade},
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{
        self,
        process::{ProcessControl, ProcessIo},
    },
};

use super::{
    daemon_fixture::TestResult,
    harness::{Lab, content, refused, succeed},
};

#[test]
fn exec_runs_typed_argv_and_returns_its_streams_and_status() -> TestResult {
    let lab = Lab::start("exec", "1")?;
    let run = lab.remote(
        &["exec"],
        &["--", "sh", "-c", "echo out; echo err >&2; exit 3"],
    )?;
    assert_eq!(run.status.code(), Some(3));
    assert_eq!(run.stdout, b"out\n");
    assert!(String::from_utf8(run.stderr)?.contains("err"));
    // Arguments are argv, never shell text.
    let literal = lab.remote(&["exec"], &["--", "echo", "$HOME;ls"])?;
    succeed(&literal)?;
    assert_eq!(literal.stdout, b"$HOME;ls\n");
    let input = content(2 << 20);
    let echoed = lab.member.piped(
        &["exec", "--endpoint", lab.owner.id(), "--stdin", "--", "cat"],
        &input,
    )?;
    succeed(&echoed)?;
    assert!(echoed.stdout == input, "stdin did not round-trip");
    refused(
        &lab.remote(&["exec"], &["--", "/nonexistent/ma2a-program"])?,
        "spawn_failed",
    )?;
    let timed = lab.remote(&["exec"], &["--timeout", "300ms", "--", "sleep", "10"])?;
    assert_eq!(timed.status.code(), Some(124));
    lab.shutdown()
}

#[test]
fn shell_carries_terminal_io_status_and_resize() -> TestResult {
    let lab = Lab::start("shell", "1")?;
    let session = lab.member.piped(
        &["shell", "--endpoint", lab.owner.id(), "--", "sh"],
        b"echo hi-$((6*7))\nexit 4\n",
    )?;
    assert_eq!(session.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&session.stdout).contains("hi-42"));

    // Resize has no CLI trigger without a terminal, so the same stream plane is driven here
    // through the daemon's local API, exactly as the CLI does.
    let runtime = tokio::runtime::Runtime::new()?;
    let shown = runtime.block_on(resized_session(&lab))?;
    assert!(shown.contains("50 132"), "{shown}");
    lab.shutdown()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        let _written = write!(text, "{byte:02x}");
        text
    })
}

async fn resized_session(lab: &Lab) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let request_id = ma2a_core::RequestId::random().map_err(|_| "random")?;
    let upgrade = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "operation": "stream_open",
        "request_id": hex(request_id.as_bytes()),
        "target_endpoint_id": lab.owner.id(),
        "service": "shell",
    }))?)?;
    let (_, target, _) = upgrade.stream_open().ok_or("upgrade")?;
    let client = LocalApiClient::new(IpcPaths::new(lab.member.state_dir())?);
    let StreamUpgrade::Opened(stream) = client.open_stream(&upgrade).await? else {
        return Err("stream refused".into());
    };
    let (read, write) = tokio::io::split(stream);
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    let spec = ProcessSpec::new(
        Some("sh".to_owned()),
        vec!["-c".to_owned(), "read line; stty size".to_owned()],
        None,
    )?;
    let request = ShellRequest::new(
        spec,
        ("dumb".to_owned(), TerminalSize::new(80, 24)?),
        60_000,
    )?;
    let open = ma2a_core::service::OpenHeader::new(request_id, target, OpenBody::Shell(request))
        .encode()?;
    writer
        .frame(ma2a_core::service::FrameKind::Open, &open)
        .await?;
    writer.flush().await?;
    let (sizes, resizes) = tokio::sync::mpsc::channel(1);
    sizes.send(TerminalSize::new(132, 50)?).await?;
    let (mut typed, input) = tokio::io::duplex(64);
    let typing = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        tokio::io::AsyncWriteExt::write_all(&mut typed, b"go\n").await
    };
    let mut output = Vec::new();
    let local = ProcessIo::<_, _, Vec<u8>> {
        input: Some(input),
        output: &mut output,
        error: None,
    };
    let control = ProcessControl {
        resizes: Some(resizes),
        cancel: std::future::pending(),
    };
    let run = client::process::run((&mut reader, &mut writer), local, control);
    let (result, typed) = tokio::join!(run, typing);
    typed?;
    assert_eq!(result?.termination, Termination::Exited(0));
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn drain_pipe(mut pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
    })
}

fn open_probed_pipe() -> Result<
    (std::io::PipeReader, std::io::PipeWriter, usize),
    Box<dyn std::error::Error + Send + Sync>,
> {
    use std::io::{Read as _, Write as _};

    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

    let (mut read_end, mut write_end) = std::io::pipe()?;
    let saved = fcntl_getfl(&write_end)?;
    fcntl_setfl(&write_end, saved | OFlags::NONBLOCK)?;
    let total = (|| -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let chunk = [0_u8; 4_096];
        let mut total = 0_usize;
        loop {
            match write_end.write(&chunk) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        loop {
            match write_end.write(&[0_u8]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(total)
    })();
    fcntl_setfl(&write_end, saved)?;
    let capacity = total?;
    let mut drained = vec![0_u8; capacity];
    read_end.read_exact(&mut drained)?;
    Ok((read_end, write_end, capacity))
}

fn wait_pipe_filled(
    child: &mut std::process::Child,
    fd: std::os::fd::BorrowedFd<'_>,
    capacity: usize,
) -> TestResult {
    let target = u64::try_from(capacity)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("exec exited before pipe filled: {status}").into());
        }
        if rustix::io::ioctl_fionread(fd)? >= target {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("exec pipe never filled".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn await_exec_exit(
    child: &mut std::process::Child,
) -> super::daemon_fixture::TestValue<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("exec hung past 5s with full pipe".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn exec_exits_promptly_on_timeout_and_sigint_with_undrained_pipes() -> TestResult {
    use std::{
        os::fd::{AsFd, OwnedFd},
        process::Stdio,
    };

    let lab = Lab::start("exec-blocked-pipes", "1")?;
    let (stderr_read, stderr_write, cap) = open_probed_pipe()?;
    let timeout_script = format!("head -c {cap} /dev/zero | tr '\\0' e >&2; sleep 30");
    let mut child = lab
        .member
        .command(&[
            "exec",
            "--endpoint",
            lab.owner.id(),
            "--timeout",
            "500ms",
            "--",
            "sh",
            "-c",
            &timeout_script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_write))
        .spawn()?;
    let stdout_drain = drain_pipe(child.stdout.take().ok_or("missing stdout")?);
    let stderr_fd: OwnedFd = stderr_read.into();
    wait_pipe_filled(&mut child, stderr_fd.as_fd(), cap)?;
    let status = await_exec_exit(&mut child)?;
    drop(stderr_fd);
    let _ = stdout_drain.join();
    assert_eq!(status.code(), Some(124), "expected 124, got {status:?}");

    for block_stdout in [true, false] {
        let (blocked_read, blocked_write, cap) = open_probed_pipe()?;
        let fill = cap.saturating_mul(4);
        let script = if block_stdout {
            format!("head -c {fill} /dev/zero | tr '\\0' a; sleep 30")
        } else {
            format!("head -c {fill} /dev/zero | tr '\\0' e >&2; sleep 30")
        };
        let mut command = lab.member.command(&[
            "exec",
            "--endpoint",
            lab.owner.id(),
            "--",
            "sh",
            "-c",
            &script,
        ]);
        command.stdin(Stdio::null());
        let other = if block_stdout {
            command
                .stdout(Stdio::from(blocked_write))
                .stderr(Stdio::piped());
            let mut spawned = command.spawn()?;
            let reader = drain_pipe(spawned.stderr.take().ok_or("missing stderr")?);
            (spawned, reader)
        } else {
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::from(blocked_write));
            let mut spawned = command.spawn()?;
            let reader = drain_pipe(spawned.stdout.take().ok_or("missing stdout")?);
            (spawned, reader)
        };
        let (mut child, other) = other;
        let blocked_fd: OwnedFd = blocked_read.into();
        wait_pipe_filled(&mut child, blocked_fd.as_fd(), cap)?;
        let pid = rustix::process::Pid::from_raw(i32::try_from(child.id())?).ok_or("no pid")?;
        rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
        let status = await_exec_exit(&mut child)?;
        drop(blocked_fd);
        let _ = other.join();
        assert_eq!(status.code(), Some(130), "expected 130, got {status:?}");
    }
    lab.shutdown()
}
