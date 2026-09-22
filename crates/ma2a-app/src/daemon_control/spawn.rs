//! Starting a detached daemon and learning what became of it.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead as _, BufReader, Read as _},
    path::Path,
    process::{ChildStdout, Command, Stdio},
};

use ma2a_runtime::ipc::IpcPaths;

use super::STARTUP_DEADLINE;
use crate::AppError;

/// The single line a detached daemon writes once it can serve commands.
pub(crate) const READY_TOKEN: &str = "ma2a-daemon-ready";

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

/// Reads the readiness line a freshly spawned daemon writes, or explains its death.
///
/// End-of-file without the line means the child exited during startup. That is
/// the moment its own diagnostics matter, so they are read back and reported
/// instead of the caller being told only that some deadline elapsed.
fn read_ready(stdout: ChildStdout, log: &Path) -> Result<(), AppError> {
    let mut line = String::new();
    let read = BufReader::new(stdout).read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::other(format!(
            "daemon exited during startup: {}",
            daemon_diagnostics(log)
        ))
        .into());
    }
    if line.trim() == READY_TOKEN {
        return Ok(());
    }
    Err(io::Error::other("daemon reported an unrecognised startup line").into())
}

/// Returns what the daemon wrote before it died, bounded for one error message.
fn daemon_diagnostics(log: &Path) -> String {
    let mut recorded = String::new();
    match File::open(log).and_then(|file| file.take(4096).read_to_string(&mut recorded)) {
        Ok(_) if !recorded.trim().is_empty() => recorded.trim().to_owned(),
        Ok(_) | Err(_) => "it recorded no reason".to_owned(),
    }
}

pub(super) async fn spawn_daemon(state_dir: &Path, paths: &IpcPaths) -> Result<(), AppError> {
    let executable = std::env::current_exe()?;
    let diagnostics = open_daemon_log(paths)?;
    let mut command = Command::new(executable);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("daemon-detached")
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
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("daemon readiness pipe was not created"))?;
    let log = paths.log_path();
    let ready = tokio::task::spawn_blocking(move || read_ready(stdout, &log));
    match tokio::time::timeout(STARTUP_DEADLINE, ready).await {
        Ok(joined) => joined.map_err(|_| io::Error::other("daemon readiness task failed"))?,
        // Nothing else owns this process yet, so it must not be left behind.
        Err(_elapsed) => {
            let _killed = child.kill();
            let _reaped = child.wait();
            Err(io::Error::new(io::ErrorKind::TimedOut, "daemon startup timed out").into())
        }
    }
}
