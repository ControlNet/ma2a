//! Process-level coverage for daemon startup convergence and shutdown.

use std::{
    error::Error,
    fs::{self, OpenOptions, TryLockError},
    io::{Read as _, Write as _},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::Value;

#[path = "daemon_lifecycle/identity.rs"]
mod identity;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    state_dir: PathBuf,
}

impl Fixture {
    fn new() -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let state_dir = std::env::temp_dir().join(format!(
            "ma2a-daemon-lifecycle-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { state_dir })
    }

    fn command(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ma2a"));
        command
            .arg("--state-dir")
            .arg(&self.state_dir)
            .arg(operation);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command
    }

    fn run(&self, operation: &str) -> std::io::Result<Output> {
        self.command(operation).output()
    }

    fn run_status(&self) -> TestResultValue<Value> {
        let output = self.run("status")?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn runtime_dir(&self) -> PathBuf {
        self.state_dir.join("run-v1")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.run("shutdown");
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.runtime_dir().join("control.sock").exists() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let _cleanup = fs::remove_dir_all(&self.state_dir);
    }
}

type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn unknown_command_reports_user_facing_error() -> TestResult {
    // Given
    let fixture = Fixture::new()?;

    // When
    let output = fixture.run("unknown-command")?;

    // Then
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("unknown command; run ma2a --help"));
    assert!(!stderr.contains("Usage("));
    Ok(())
}

#[test]
fn status_autostarts_one_private_daemon() -> TestResult {
    // Given
    let fixture = Fixture::new()?;

    // When
    let response = fixture.run_status()?;

    // Then
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("status")
    );
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(fixture.runtime_dir())?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.runtime_dir().join("control.sock"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Ok(())
}

#[test]
fn concurrent_status_calls_converge_on_one_daemon() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let children = (0..20)
        .map(|_| fixture.command("status").spawn())
        .collect::<Result<Vec<_>, _>>()?;

    // When
    let outputs = children
        .into_iter()
        .map(std::process::Child::wait_with_output)
        .collect::<Result<Vec<_>, _>>()?;

    // Then
    assert!(outputs.iter().all(|output| output.status.success()));
    for output in outputs {
        let response: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(
            response.pointer("/result/type").and_then(Value::as_str),
            Some("status")
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    identity::assert_live_endpoint_matches_persisted(&fixture.state_dir)?;
    Ok(())
}

#[test]
fn autostart_reclaims_stale_socket_while_holding_startup_lock() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.runtime_dir())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(fixture.runtime_dir(), fs::Permissions::from_mode(0o700))?;
    }
    fs::write(fixture.runtime_dir().join("control.sock"), b"stale")?;

    // When
    let response = fixture.run_status()?;

    // Then
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("status")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        assert!(
            fs::metadata(fixture.runtime_dir().join("control.sock"))?
                .file_type()
                .is_socket()
        );
    }
    Ok(())
}

#[test]
fn graceful_shutdown_releases_singleton_and_removes_endpoint() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let _status = fixture.run_status()?;

    // When
    let output = fixture.run("shutdown")?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("shutting_down")
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let endpoint = fixture.runtime_dir().join("control.sock");
    while endpoint.exists() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(!endpoint.exists());
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    lock.try_lock()
        .map_err(|error| format!("daemon lock remained held: {error:?}"))?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn incompatible_daemon_reports_version_mismatch_without_startup_timeout() -> TestResult {
    use std::os::unix::{fs::PermissionsExt as _, net::UnixListener};

    // Given
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.runtime_dir())?;
    fs::set_permissions(fixture.runtime_dir(), fs::Permissions::from_mode(0o700))?;
    let daemon_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    daemon_lock.lock()?;
    let endpoint = fixture.runtime_dir().join("control.sock");
    let listener = UnixListener::bind(&endpoint)?;
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let server_stopped = Arc::clone(&stopped);
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        while !server_stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    let mut header = [0_u8; 12];
                    stream.read_exact(&mut header)?;
                    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                    let mut payload = vec![0_u8; usize::try_from(length).unwrap_or(0)];
                    stream.read_exact(&mut payload)?;
                    let response = br#"{"version":2,"result":{"type":"handshake"}}"#;
                    stream.write_all(&u32::try_from(response.len()).unwrap_or(0).to_be_bytes())?;
                    stream.write_all(&header[4..])?;
                    stream.write_all(response)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now();
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    });

    // When
    let started = Instant::now();
    let output = fixture.run("status")?;

    // Then
    stopped.store(true, Ordering::Relaxed);
    server
        .join()
        .map_err(|_| "incompatible daemon server panicked")??;
    fs::remove_file(endpoint)?;
    drop(daemon_lock);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("handshake version mismatch"));
    assert!(started.elapsed() < Duration::from_secs(3));
    Ok(())
}
