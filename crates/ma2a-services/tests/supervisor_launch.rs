//! Pre-proof failures and cancellations in `supervisor::launch::spawn`.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use ma2a_core::service::{ProcessId, ProcessSpec, ProcessStart};
use ma2a_services::supervisor::{
    Credentials, Launch, SupervisorAddress,
    launch::{READY_DEADLINE, spawn},
};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use support::{TempRoot, TestResult};

const MIB: u32 = 1_048_576;

struct PidGuard(PathBuf);

impl Drop for PidGuard {
    fn drop(&mut self) {
        if let Some(pid) = read_pid(&self.0)
            && let Some(pid) = Pid::from_raw(pid)
        {
            let _killed = kill_process(pid, Signal::KILL);
        }
    }
}

fn read_pid(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    if !text.ends_with('\n') {
        return None;
    }
    text.trim().parse().ok()
}

fn is_present(pid: i32) -> bool {
    Pid::from_raw(pid).is_some_and(|pid| test_kill_process(pid).is_ok())
}

async fn wait_for_pid(path: &Path) -> TestResult<i32> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = read_pid(path) {
            return Ok(pid);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "helper never wrote pid file"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn make_launch(root: &TempRoot, address: String) -> TestResult<Launch> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let mut secret = [0; 32];
    getrandom::fill(&mut secret)?;
    Ok(Launch {
        credentials: Credentials::new(ProcessId::random()?, secret),
        directory: root.path().join("process"),
        address: SupervisorAddress::new(address),
        start: ProcessStart::new(spec, false, MIB)?,
    })
}

fn sh_args(script: &str, pidfile: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-c"),
        OsString::from(script),
        OsString::from("_"),
        pidfile.as_os_str().to_owned(),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_bounds_write_and_cleans_up_when_helper_never_reads_stdin() -> TestResult {
    let root = TempRoot::new("launch-unread")?;
    let pidfile = root.path().join("child.pid");
    let _guard = PidGuard(pidfile.clone());
    // Exceed Linux 64 KiB pipe capacity so `stdin.write_all` blocks if the helper never reads.
    let launch = make_launch(&root, "x".repeat(131_072))?;
    let args = sh_args("echo $$ > \"$1\"; exec sleep 60", &pidfile);

    let res = tokio::time::timeout(
        READY_DEADLINE + Duration::from_secs(2),
        spawn((Path::new("/bin/sh"), &args), &launch),
    )
    .await
    .map_err(|_| "spawn blocked past READY_DEADLINE when helper never read stdin")?;
    assert!(res.is_err(), "expected spawn failure, got Ok");

    let pid = wait_for_pid(&pidfile).await?;
    assert!(
        !is_present(pid),
        "unproven helper {pid} remained alive or unreaped after spawn failure"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_cleans_up_when_helper_reads_stdin_but_never_proves() -> TestResult {
    let root = TempRoot::new("launch-unproven")?;
    let pidfile = root.path().join("child.pid");
    let _guard = PidGuard(pidfile.clone());
    let address = root.path().join("never.sock");
    let launch = make_launch(&root, address.to_str().ok_or("path")?.to_owned())?;
    let args = sh_args("echo $$ > \"$1\"; cat >/dev/null; exec sleep 60", &pidfile);

    let res = tokio::time::timeout(
        READY_DEADLINE + Duration::from_secs(7),
        spawn((Path::new("/bin/sh"), &args), &launch),
    )
    .await
    .map_err(|_| "spawn blocked past deadline")?;
    assert!(res.is_err(), "expected spawn failure, got Ok");

    let pid = wait_for_pid(&pidfile).await?;
    assert!(
        !is_present(pid),
        "unproven helper {pid} remained alive or unreaped after spawn timeout"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_cancellation_terminates_and_reaps_unproven_child() -> TestResult {
    let root = TempRoot::new("launch-cancel")?;
    let pidfile = root.path().join("child.pid");
    let _guard = PidGuard(pidfile.clone());
    let address = root.path().join("cancel.sock");
    let launch = make_launch(&root, address.to_str().ok_or("path")?.to_owned())?;
    let args = sh_args("echo $$ > \"$1\"; cat >/dev/null; exec sleep 60", &pidfile);

    let pid = {
        let spawning = spawn((Path::new("/bin/sh"), &args), &launch);
        tokio::pin!(spawning);
        tokio::select! {
            res = &mut spawning => {
                return Err(format!("spawn finished unexpectedly: {res:?}").into());
            }
            pid = wait_for_pid(&pidfile) => pid?,
        }
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while is_present(pid) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled unproven helper {pid} remained alive or unreaped"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}
