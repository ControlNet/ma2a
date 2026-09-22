//! Starting a detached daemon, and owning it until it proves what it became.
//!
//! A launcher that reports failure while its child is still alive is how orphan
//! daemons are made, so this holds the child handle across the entire attempt.
//! Either the child reports readiness for *this* launch — and only then is it
//! released to run on its own — or it is terminated and reaped here.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead as _, BufReader, Read as _},
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    time::Duration,
};

use ma2a_runtime::ipc::{DaemonReport, IpcPaths, random_launch_nonce};

use super::STARTUP_DEADLINE;
use crate::AppError;

/// Carries one launch's identity to the daemon it starts.
///
/// The environment rather than argv, because argv is world-readable on Unix and
/// this value is what tells two concurrent launches apart.
pub(crate) const LAUNCH_NONCE_VARIABLE: &str = "MA2A_LAUNCH_NONCE";

/// How long a child gets to exit after being asked before it is forced.
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const TERMINATION_POLL: Duration = Duration::from_millis(20);

/// Bounds the readiness record, which has a fixed shape and a known size.
const READINESS_RECORD_LIMIT: u64 = 8 * 1024;

/// Launches a daemon and returns only once it has identified itself.
///
/// # Errors
/// Returns the daemon's own recorded reason when it died during startup, and a
/// timed-out error when it neither reported readiness nor exited. In every
/// failing case the launched process has been terminated and reaped first.
pub(super) async fn spawn_daemon(
    state_dir: &Path,
    paths: &IpcPaths,
) -> Result<DaemonReport, AppError> {
    let nonce = random_launch_nonce()?;
    let mut child = launch(state_dir, paths, &nonce)?;
    match verified_ready(&mut child, paths, &nonce).await {
        // The daemon reported readiness on the pipe, so it is now its own owner.
        Ok(report) => Ok(report),
        Err(error) => {
            terminate(child).await;
            Err(error)
        }
    }
}

fn launch(state_dir: &Path, paths: &IpcPaths, nonce: &str) -> Result<Child, AppError> {
    let executable = std::env::current_exe()?;
    let diagnostics = open_daemon_log(paths)?;
    let mut command = Command::new(executable);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("daemon-detached")
        .env(LAUNCH_NONCE_VARIABLE, nonce)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(diagnostics));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    Ok(command.spawn()?)
}

async fn verified_ready(
    child: &mut Child,
    paths: &IpcPaths,
    nonce: &str,
) -> Result<DaemonReport, AppError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("daemon readiness pipe was not created"))?;
    let log = paths.log_path();
    let reading = tokio::task::spawn_blocking(move || read_ready(stdout, &log));
    let report = match tokio::time::timeout(STARTUP_DEADLINE, reading).await {
        Ok(joined) => joined.map_err(|_| io::Error::other("daemon readiness task failed"))??,
        Err(_elapsed) => {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "daemon startup timed out").into());
        }
    };
    // Readiness that belongs to another launch proves nothing about this one.
    if report.launch_nonce() == Some(nonce) {
        Ok(report)
    } else {
        Err(io::Error::other("a daemon reported readiness for a different launch").into())
    }
}

/// Reads the readiness record a freshly launched daemon writes, or explains its death.
///
/// End-of-file without a record means the child exited during startup. That is
/// the moment its own diagnostics matter, so they are read back and reported
/// instead of the caller being told only that some deadline elapsed.
fn read_ready(stdout: ChildStdout, log: &Path) -> Result<DaemonReport, AppError> {
    let mut line = String::new();
    // The record has a fixed shape and a known size, so a reader that never sees
    // a newline is reading something else and must not grow without bound.
    let read = BufReader::new(stdout)
        .take(READINESS_RECORD_LIMIT)
        .read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::other(format!(
            "daemon exited during startup: {}",
            daemon_diagnostics(log)
        ))
        .into());
    }
    DaemonReport::parse(line.as_bytes())
        .filter(|report| report.operation() == "ready")
        .ok_or_else(|| io::Error::other("daemon reported an unrecognised startup line").into())
}

/// Ends the exact process this launch created and waits until it is reaped.
///
/// Nothing else has ever owned this child, so leaving it is leaving an orphan.
/// It is asked to stop first and forced only if it does not, because a daemon
/// part-way through startup may still hold a half-written Runtime.
async fn terminate(mut child: Child) {
    request_exit(&child);
    if !exited_within_grace(&mut child).await {
        let _killed = child.kill();
    }
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

/// Windows offers no graceful signal to a detached process group, so forcing is
/// the only termination available and the grace wait simply finds it already gone.
#[cfg(windows)]
const fn request_exit(_child: &Child) {}

async fn exited_within_grace(child: &mut Child) -> bool {
    let deadline = tokio::time::Instant::now() + TERMINATION_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return true,
            Ok(None) => {}
            Err(_unknown) => return false,
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(TERMINATION_POLL).await;
    }
}

/// Opens the owner-private file that keeps a detached daemon's diagnostics.
///
/// A detached daemon has no terminal, and discarding its standard error throws
/// away the only account of why it refused to start or why it stopped serving.
/// The file is truncated for each run, so it describes the current daemon rather
/// than growing without bound, and it lives in the owner-only runtime directory
/// because it can name local paths.
fn open_daemon_log(paths: &IpcPaths) -> Result<File, AppError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok(options.open(paths.log_path())?)
}

/// Returns what the daemon wrote before it died, bounded for one error message.
fn daemon_diagnostics(log: &Path) -> String {
    let mut recorded = String::new();
    match File::open(log).and_then(|file| file.take(4096).read_to_string(&mut recorded)) {
        Ok(_) if !recorded.trim().is_empty() => recorded.trim().to_owned(),
        Ok(_) | Err(_) => "it recorded no reason".to_owned(),
    }
}
