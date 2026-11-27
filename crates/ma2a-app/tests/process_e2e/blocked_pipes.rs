//! Real child-process tests: `process attach` (without `--stdin`) and `process logs --follow`
//! must exit promptly on SIGINT even when stdout or stderr is an undrained full pipe.

use std::{
    io::Read,
    os::fd::AsFd,
    process::{Child, ExitStatus},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rustix::process::Signal;

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::text,
    lab::{ProcessLab, signal},
};

#[derive(Clone, Copy)]
enum BlockedStream {
    Stdout,
    Stderr,
}

fn drain_pipe(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _read = pipe.read_to_end(&mut bytes);
        bytes
    })
}

fn wait_pipe_full(fd: impl AsFd, child: &mut Child) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = 0_u64;
    let mut stable_since: Option<Instant> = None;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("child exited before filling pipe: {status}").into());
        }
        let available = rustix::io::ioctl_fionread(&fd)?;
        if available >= 65_536 {
            return Ok(());
        }
        if available >= 16_384 && available == last {
            if stable_since.is_some_and(|since| since.elapsed() >= Duration::from_millis(100)) {
                return Ok(());
            }
            stable_since.get_or_insert_with(Instant::now);
        } else {
            last = available;
            stable_since = None;
        }
        if Instant::now() >= deadline {
            return Err(format!("pipe never filled (buffered {available} bytes)").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn await_exit(child: &mut Child, limit: Duration) -> TestValue<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _killed = child.kill();
            let _reaped = child.wait();
            return Err(format!("CLI hung past {} ms after SIGINT", limit.as_millis()).into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_blocked_case(
    (lab, id): (&ProcessLab, &str),
    subcommand: &[&str],
    blocked: BlockedStream,
) -> TestResult {
    let mut child = lab.spawn(subcommand)?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (other_reader, status) = match blocked {
        BlockedStream::Stdout => {
            let reader = drain_pipe(stderr);
            wait_pipe_full(&stdout, &mut child)?;
            signal(&child, Signal::INT)?;
            let status = await_exit(&mut child, Duration::from_secs(5));
            drop(stdout);
            (reader, status?)
        }
        BlockedStream::Stderr => {
            let reader = drain_pipe(stdout);
            wait_pipe_full(&stderr, &mut child)?;
            signal(&child, Signal::INT)?;
            let status = await_exit(&mut child, Duration::from_secs(5));
            drop(stderr);
            (reader, status?)
        }
    };
    let _other = other_reader.join().map_err(|_| "drain thread panicked")?;
    assert_eq!(
        status.code(),
        Some(0),
        "expected detach exit 0, got {status:?}"
    );
    assert_eq!(
        text(&lab.json(&["inspect", id])?, "/process/state"),
        "running"
    );
    Ok(())
}

#[test]
fn attach_and_logs_follow_exit_0_on_sigint_with_undrained_stdout_or_stderr() -> TestResult {
    let lab = ProcessLab::start("blocked-pipes")?;

    // 1. `process attach` (no --stdin) with undrained stdout.
    let stdout_flood = "head -c 262144 /dev/zero | tr '\\0' a; sleep 30";
    let id = lab.start_process(&["--", "sh", "-c", stdout_flood])?;
    lab.wait_state(&id, "running")?;
    run_blocked_case(
        (&lab, &id),
        &["attach", &id, "--from", "start"],
        BlockedStream::Stdout,
    )?;
    lab.json(&["terminate", &id, "--grace", "500ms"])?;

    // 2. `process attach` (no --stdin) with undrained stderr.
    let stderr_flood = "head -c 262144 /dev/zero | tr '\\0' e >&2; sleep 30";
    let id = lab.start_process(&["--", "sh", "-c", stderr_flood])?;
    lab.wait_state(&id, "running")?;
    run_blocked_case(
        (&lab, &id),
        &["attach", &id, "--from", "start"],
        BlockedStream::Stderr,
    )?;
    lab.json(&["terminate", &id, "--grace", "500ms"])?;

    // 3. `process logs --from now --follow` (live phase) with undrained stdout.
    let live_stdout_flood = "i=0; while [ $i -lt 300 ]; do head -c 16384 /dev/zero | tr '\\0' a; i=$((i+1)); sleep 0.02; done";
    let id = lab.start_process(&["--", "sh", "-c", live_stdout_flood])?;
    lab.wait_state(&id, "running")?;
    run_blocked_case(
        (&lab, &id),
        &["logs", &id, "--from", "now", "--follow"],
        BlockedStream::Stdout,
    )?;
    lab.json(&["terminate", &id, "--grace", "500ms"])?;

    // 4. `process logs --from now --follow` (live phase) with undrained stderr.
    let live_stderr_flood = "i=0; while [ $i -lt 300 ]; do head -c 16384 /dev/zero | tr '\\0' e >&2; i=$((i+1)); sleep 0.02; done";
    let id = lab.start_process(&["--", "sh", "-c", live_stderr_flood])?;
    lab.wait_state(&id, "running")?;
    run_blocked_case(
        (&lab, &id),
        &["logs", &id, "--from", "now", "--follow"],
        BlockedStream::Stderr,
    )?;
    lab.json(&["terminate", &id, "--grace", "500ms"])?;

    lab.shutdown()
}
