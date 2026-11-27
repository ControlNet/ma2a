//! Child-process regression tests for `service call` with undrained stdout/stderr pipes.
#![cfg(unix)]

use std::{
    error::Error,
    fmt::Write as _,
    fs::File,
    os::fd::{AsFd, OwnedFd},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use ma2a_core::service::{
    Channel, EMPTY_ACCEPT, FrameKind, ServiceError, encode_call_accept, encode_call_outcome,
};
use ma2a_runtime::{
    api::{LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES},
    ipc::IpcPaths,
};
use ma2a_services::{FrameReader, FrameWriter};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    runtime::Runtime,
};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

struct Standin {
    state_dir: PathBuf,
    endpoint: String,
    _lock: File,
    _runtime: Runtime,
}

impl Drop for Standin {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

#[derive(Clone, Copy)]
enum Mode {
    CancelChannel(Channel),
    OkStdoutOnCancel,
    DropChannelOnCancel(Channel),
}

fn start_standin(label: &str, mode: Mode) -> TestResult<Standin> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let state_dir = std::env::temp_dir().join(format!(
        "ma2a-svc-blocked-{label}-{:016x}",
        u64::from_le_bytes(nonce)
    ));
    std::fs::create_dir(&state_dir)?;
    ma2a_store::protect_private_directory(&state_dir, "test state directory")?;
    let paths = IpcPaths::new(&state_dir)?;
    paths.prepare()?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.lock_path())?;
    lock.lock()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let listener = runtime.block_on(async { UnixListener::bind(paths.socket_path()) })?;
    let mut endpoint = String::with_capacity(64);
    for byte in iroh::SecretKey::generate().public().as_bytes() {
        let _written = write!(endpoint, "{byte:02x}");
    }
    runtime.spawn(serve(listener, mode));
    Ok(Standin {
        state_dir,
        endpoint,
        _lock: lock,
        _runtime: runtime,
    })
}

async fn serve(listener: UnixListener, mode: Mode) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let Ok((correlation, request)) = read_frame(&mut stream).await else {
            continue;
        };
        let answer = match request.get("operation").and_then(Value::as_str) {
            _ if request.get("ma2a_lifecycle").is_some() => {
                json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "invalid_request"})
            }
            Some("handshake") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "handshake", "payload": {}},
            }),
            Some("stream_open") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "stream_opened", "payload": {}},
            }),
            _ => {
                json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "invalid_input"})
            }
        };
        if write_frame(&mut stream, correlation, &answer)
            .await
            .is_err()
        {
            continue;
        }
        if answer.pointer("/result/type") == Some(&json!("stream_opened")) {
            tokio::spawn(handle_call(stream, mode));
        }
    }
}

async fn handle_call(stream: UnixStream, mode: Mode) {
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    let Ok(Some(open)) = reader.next().await else {
        return;
    };
    if open.kind() != FrameKind::Open {
        return;
    }
    let accept = match mode {
        Mode::CancelChannel(_) | Mode::OkStdoutOnCancel => {
            encode_call_accept(&[]).unwrap_or_default()
        }
        Mode::DropChannelOnCancel(_) => EMPTY_ACCEPT.to_vec(),
    };
    if writer.frame(FrameKind::Accept, &accept).await.is_err() {
        return;
    }
    let channel = match mode {
        Mode::CancelChannel(channel) | Mode::DropChannelOnCancel(channel) => channel,
        Mode::OkStdoutOnCancel => Channel::Output,
    };
    let chunk = vec![b'x'; 8_192];
    for _ in 0..32 {
        if writer.data(channel, &chunk).await.is_err() {
            return;
        }
    }
    if writer.flush().await.is_err() {
        return;
    }
    while let Ok(Some(frame)) = reader.next().await {
        if frame.kind() == FrameKind::Cancel {
            let outcome = match mode {
                Mode::CancelChannel(_) => encode_call_outcome(Err(ServiceError::Cancelled)),
                Mode::OkStdoutOnCancel => encode_call_outcome(Ok(b"{\"ok\":true}")),
                Mode::DropChannelOnCancel(_) => return,
            };
            let Ok(payload) = outcome else {
                return;
            };
            let _ = writer.frame(FrameKind::Outcome, &payload).await;
            let _ = writer.flush().await;
            return;
        }
    }
}

async fn read_frame(stream: &mut UnixStream) -> TestResult<(u64, Value)> {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).await?;
    let (length, correlation) = header.split_at(4);
    let length = usize::try_from(u32::from_be_bytes(length.try_into()?))?;
    if length > MAX_LOCAL_REQUEST_BYTES {
        return Err("request too large".into());
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    Ok((
        u64::from_be_bytes(correlation.try_into()?),
        serde_json::from_slice(&payload)?,
    ))
}

async fn write_frame(stream: &mut UnixStream, correlation: u64, answer: &Value) -> TestResult {
    let payload = serde_json::to_vec(answer)?;
    let mut frame = u32::try_from(payload.len())?.to_be_bytes().to_vec();
    frame.extend_from_slice(&correlation.to_be_bytes());
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await?;
    Ok(())
}

fn spawn_service_call(standin: &Standin, use_stdin: bool) -> TestResult<Child> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ma2a"));
    command.arg("--state-dir").arg(&standin.state_dir).args([
        "service",
        "call",
        "--endpoint",
        &standin.endpoint,
        "--version",
        "1",
        "example",
        "stream",
    ]);
    if use_stdin {
        command.arg("--stdin").stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    Ok(command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}

fn drain(mut pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

fn wait_pipe_full_and_interrupt(
    label: &str,
    child: &mut Child,
    block_stdout: bool,
) -> TestResult<std::process::ExitStatus> {
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (blocked_fd, other): (OwnedFd, _) = if block_stdout {
        (stdout.into(), drain(stderr))
    } else {
        (stderr.into(), drain(stdout))
    };
    let fill_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            drop(blocked_fd);
            let drained = other.join().unwrap_or_default();
            return Err(format!(
                "{label}: exited before pipe filled: {status}; other={}",
                String::from_utf8_lossy(&drained)
            )
            .into());
        }
        if rustix::io::ioctl_fionread(blocked_fd.as_fd())? >= 65_536 {
            break;
        }
        if Instant::now() >= fill_deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label}: pipe never filled").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = Pid::from_raw(i32::try_from(child.id())?).ok_or("no pid")?;
    kill_process(pid, Signal::INT)?;
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label}: hung past 5s after SIGINT").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(blocked_fd);
    let _ = other.join();
    Ok(status)
}

#[test]
fn service_call_exits_promptly_on_sigint_with_undrained_stdout_or_stderr() -> TestResult {
    // 1. No --stdin, undrained stdout, Cancel -> Cancelled.
    let standin = start_standin("stdout", Mode::CancelChannel(Channel::Output))?;
    let mut child = spawn_service_call(&standin, false)?;
    let status = wait_pipe_full_and_interrupt("service-stdout", &mut child, true)?;
    assert!(!status.success(), "expected failure exit, got {status:?}");

    // 2. No --stdin, undrained stderr, Cancel -> Cancelled.
    let standin = start_standin("stderr", Mode::CancelChannel(Channel::Error))?;
    let mut child = spawn_service_call(&standin, false)?;
    let status = wait_pipe_full_and_interrupt("service-stderr", &mut child, false)?;
    assert!(!status.success(), "expected failure exit, got {status:?}");

    // 3. Even with --stdin, if target answers Ok(Called) after stdout was interrupted on a full
    // pipe, the CLI must not block locking std::io::stdout().
    let standin = start_standin("ok-on-cancel", Mode::OkStdoutOnCancel)?;
    let mut child = spawn_service_call(&standin, true)?;
    let status = wait_pipe_full_and_interrupt("service-ok-on-cancel", &mut child, true)?;
    assert!(
        !status.success(),
        "expected interrupted exit, got {status:?}"
    );

    // 4. `exec` (no --stdin) returning Err(OutcomeUnknown) on cancel with full stdout/stderr pipe.
    for (label, channel, block_stdout) in [
        ("exec-stdout", Channel::Output, true),
        ("exec-stderr", Channel::Error, false),
    ] {
        let standin = start_standin(label, Mode::DropChannelOnCancel(channel))?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&standin.state_dir)
            .args(["exec", "--endpoint", &standin.endpoint, "--", "sleep", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let status = wait_pipe_full_and_interrupt(label, &mut child, block_stdout)?;
        assert_eq!(
            status.code(),
            Some(1),
            "expected OutcomeUnknown exit 1, got {status:?}"
        );
    }
    Ok(())
}
