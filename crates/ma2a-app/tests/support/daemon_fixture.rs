//! The one place that owns a daemon process while a test needs one.
//!
//! Tests used to start a detached daemon with `ma2a start`, ignore whether that
//! worked, and remove the state directory from `Drop`. When the daemon was in
//! fact alive, that removal unlinked its socket and its lock out from under it,
//! leaving a process that no lifecycle command could address any more. Every
//! part of that is fixed here and nowhere else: the fixture owns the child,
//! tears it down explicitly, proves the ownership is gone, and only then removes
//! the directory.
//!
//! Ordinary command-surface tests take [`DaemonFixture::running`], which runs
//! the canonical foreground `ma2a daemon` as a child of the test process. Only
//! tests that are about `start`, `stop` and `restart` themselves create a real
//! detached daemon, because only they are about that.
//!
//! What this guarantees, and what it cannot: when a test returns, fails with an
//! error, or panics and unwinds, the fixture stops its daemon, reaps it, proves
//! the daemon lock is free, and only then removes the state directory. If the
//! lock cannot be proven free, the directory is left in place and the test
//! fails. None of that can run if the test process itself is killed outright —
//! `SIGKILL`, a runner crash, a machine crash — because nothing in a process
//! runs after that. A daemon left behind then survives unless something outside
//! the test ends it: nextest terminating a timed-out test's process group ends
//! the foreground daemon, which shares that group, but not a detached one, which
//! runs in a session of its own. Nothing here claims otherwise.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of the shared fixture"
)]

use std::{
    error::Error,
    fs::{self, OpenOptions, TryLockError},
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub(crate) type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
pub(crate) type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

/// How long a daemon gets to leave on request before it is forced.
const TERMINATION_GRACE: Duration = Duration::from_secs(10);
/// How long the state directory may stay owned after the daemon was stopped.
const RELEASE_DEADLINE: Duration = Duration::from_secs(30);
/// How long a launched daemon gets to report readiness.
const READINESS_DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(20);

/// A private state directory, and the daemon this test is responsible for.
pub(crate) struct DaemonFixture {
    state_dir: PathBuf,
    owned: Option<Child>,
    detached: bool,
    finished: bool,
}

impl DaemonFixture {
    /// Creates a private state directory and starts nothing.
    pub(crate) fn idle(name: &str) -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let state_dir =
            std::env::temp_dir().join(format!("ma2a-{name}-{}-{serial}", std::process::id()));
        fs::create_dir(&state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            state_dir,
            owned: None,
            detached: false,
            finished: false,
        })
    }

    /// Creates the directory and runs a foreground daemon this test owns.
    pub(crate) fn running(name: &str) -> TestValue<Self> {
        let mut fixture = Self::idle(name)?;
        fixture.start_owned()?;
        Ok(fixture)
    }

    /// Runs `ma2a daemon` as a child of this test and waits for its readiness record.
    ///
    /// Readiness arrives on the daemon's own pipe, so this returns when the
    /// daemon is genuinely serving rather than after a guessed interval, and a
    /// daemon that dies during startup ends the wait at once with end-of-file.
    pub(crate) fn start_owned(&mut self) -> TestValue<()> {
        // Its diagnostics go to a file rather than a pipe. Nothing reads the
        // daemon's standard error for the life of the test, and a pipe nobody
        // drains eventually blocks the process writing to it.
        let diagnostics = fs::File::create(self.state_dir.join("foreground-daemon.log"))?;
        let child = self
            .command(&["daemon"])
            .stdout(Stdio::piped())
            .stderr(Stdio::from(diagnostics))
            .spawn()?;
        self.adopt_owned_child(child)
    }

    /// Waits for an already spawned foreground daemon and takes responsibility for it.
    pub(crate) fn adopt_owned_child(&mut self, mut child: Child) -> TestValue<()> {
        match await_readiness(&mut child) {
            Ok(_record) => {
                self.owned = Some(child);
                Ok(())
            }
            Err(error) => {
                stop_child(&mut child);
                let recorded = fs::read_to_string(self.state_dir.join("foreground-daemon.log"))
                    .unwrap_or_default();
                Err(format!("{error}: {}", recorded.trim()).into())
            }
        }
    }

    /// Records that a detached daemon was started by other means than
    /// [`Self::start_detached`], so that teardown still stops it.
    pub(crate) const fn adopt_detached(&mut self) {
        self.detached = true;
    }

    /// Starts a real detached daemon, for the tests that are about doing so.
    pub(crate) fn start_detached(&mut self) -> TestValue<Output> {
        let output = self.run(&["start"])?;
        self.detached = true;
        Ok(output)
    }

    pub(crate) fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub(crate) fn runtime_dir(&self) -> PathBuf {
        self.state_dir.join("run-v1")
    }

    /// Builds a command against this fixture's state directory.
    pub(crate) fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ma2a"));
        command
            .arg("--state-dir")
            .arg(&self.state_dir)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub(crate) fn run(&self, arguments: &[&str]) -> TestValue<Output> {
        Ok(self.command(arguments).output()?)
    }

    /// Runs a command and fails the test with the daemon's own words if it refused.
    pub(crate) fn run_ok(&self, arguments: &[&str]) -> TestValue<Output> {
        let output = self.run(arguments)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(format!(
                "ma2a {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
    }

    pub(crate) fn json(&self, arguments: &[&str]) -> TestValue<serde_json::Value> {
        let output = self.run_ok(arguments)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    /// Ends the owned daemon and proves the directory is free, keeping it in place.
    ///
    /// For tests that have to reach the store directly between two daemon runs:
    /// opening it while a daemon still owns it is exactly the race this fixture
    /// exists to remove.
    pub(crate) fn stop_owned(&mut self) -> TestResult {
        if let Some(mut child) = self.owned.take() {
            stop_child(&mut child);
        }
        prove_released(&self.runtime_dir())
    }

    /// Returns the process identifier of the daemon this test owns, if any.
    pub(crate) fn owned_pid(&self) -> Option<u32> {
        self.owned.as_ref().map(Child::id)
    }

    /// Reports whether the daemon this test owns is still running.
    pub(crate) fn owned_daemon_is_running(&mut self) -> bool {
        self.owned
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// Ends the daemon, proves the state directory is free, and removes it.
    ///
    /// This is the path every test should take. `Drop` does the same thing, but
    /// only as a rescue for a test that returned early.
    pub(crate) fn shutdown(mut self) -> TestResult {
        self.teardown()
    }

    fn teardown(&mut self) -> TestResult {
        self.finished = true;
        if let Some(mut child) = self.owned.take() {
            stop_child(&mut child);
        } else if self.detached {
            // Exercising `stop` here is deliberate: these fixtures exist for the
            // tests that own a real detached daemon, and it is the only way to
            // end one. Whether it succeeded is settled by the proof below.
            let _stopped = self.run(&["stop"]);
        }
        prove_released(&self.runtime_dir())?;
        fs::remove_dir_all(&self.state_dir)?;
        Ok(())
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(error) = self.teardown() {
            eprintln!("daemon fixture teardown failed: {error}");
            assert!(
                std::thread::panicking(),
                "daemon fixture teardown failed: {error}"
            );
        }
    }
}

/// Waits for the daemon's readiness record on the pipe it inherited.
fn await_readiness(child: &mut Child) -> TestValue<String> {
    let stdout = child.stdout.take().ok_or("daemon readiness pipe missing")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let _reader = std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        let _delivered = sender.send(read.map(|count| (count, line)));
    });
    match receiver.recv_timeout(READINESS_DEADLINE) {
        Ok(Ok((0, _empty))) => Err("the daemon exited during startup".into()),
        Ok(Ok((_read, line))) => Ok(line),
        Ok(Err(error)) => Err(error.into()),
        Err(_timeout) => Err("the daemon never reported readiness".into()),
    }
}

/// Asks the owned daemon to stop, forces it if it will not, and reaps it.
fn stop_child(child: &mut Child) {
    request_exit(child);
    let deadline = Instant::now() + TERMINATION_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            break;
        }
        std::thread::sleep(POLL);
    }
    let _forced = child.kill();
    let _reaped = child.wait();
}

#[cfg(unix)]
fn request_exit(child: &Child) {
    use rustix::process::{Pid, Signal, kill_process};

    if let Ok(raw) = i32::try_from(child.id())
        && let Some(pid) = Pid::from_raw(raw)
    {
        let _requested = kill_process(pid, Signal::TERM);
    }
}

#[cfg(windows)]
fn request_exit(_child: &Child) {}

/// Proves that nothing owns the state directory any more.
///
/// The daemon holds its lock file open for the whole life of its process, so
/// taking that lock is the operating system reporting that the process has gone.
/// Until it does, the directory must stay exactly where it is: removing it is
/// what turns a daemon that is merely slow to exit into one nothing can address.
fn prove_released(runtime_dir: &Path) -> TestResult {
    let lock_path = runtime_dir.join("daemon.lock");
    if !lock_path.exists() {
        return Ok(());
    }
    let deadline = Instant::now() + RELEASE_DEADLINE;
    loop {
        let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "a daemon still owns {}; the state directory was left in place for diagnosis",
                runtime_dir.display()
            )
            .into());
        }
        std::thread::sleep(POLL);
    }
}
