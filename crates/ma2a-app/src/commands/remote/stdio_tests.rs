//! Subprocess regression tests for [`StdioGuard`] exit behavior on full pipes.

use std::{
    error::Error,
    io::{BufRead as _, BufReader, PipeReader, PipeWriter, Read as _, Write as _},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process},
};
use tokio::io::AsyncWriteExt as _;

use super::StdioGuard;
use crate::{AppError, commands::remote::process::interrupted};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const CHILD_ENV: &str = "MA2A_TEST_STDIO_C07_CHILD";
const TEST_NAME: &str = "commands::remote::stdio::tests::interrupted_newline_free_stdout_exits_promptly_when_pipe_is_full";

fn drain_nonblocking(read_end: &mut PipeReader) -> TestResult {
    let saved = fcntl_getfl(&*read_end)?;
    fcntl_setfl(&*read_end, saved | OFlags::NONBLOCK)?;
    let mut buf = [0_u8; 4_096];
    let result = loop {
        match read_end.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break Ok(()),
            Err(error) => break Err(error.into()),
        }
    };
    fcntl_setfl(&*read_end, saved)?;
    result
}

fn fill_nonblocking(write_end: &mut PipeWriter) -> TestResult<usize> {
    let saved = fcntl_getfl(&*write_end)?;
    fcntl_setfl(&*write_end, saved | OFlags::NONBLOCK)?;
    let result = (|| -> TestResult<usize> {
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
    fcntl_setfl(&*write_end, saved)?;
    result
}

fn run_child() -> TestResult {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let _ = std::io::stdout().flush();
        let mut stderr = std::io::stderr();
        stderr.write_all(b"STARTED\n")?;
        stderr.flush()?;
        let mut go = [0_u8; 1];
        let _ = std::io::stdin().read(&mut go)?;

        let mut interrupt = Box::pin(interrupted());
        tokio::select! {
            biased;
            () = interrupt.as_mut() => {}
            () = tokio::task::yield_now() => {}
        }

        let stdio = StdioGuard::default();
        let mut stdout = stdio.stdout();
        stdout.write_all(b"{\"ok\":true}").await?;
        // Let any std::io::stdout() line-buffer write worker finish and release the stdout lock
        // while flush() has not started yet.
        tokio::time::sleep(Duration::from_millis(20)).await;

        stderr.write_all(b"READY\n")?;
        stderr.flush()?;

        interrupt.as_mut().await;
        stdio
            .exit_error(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "standard output delivery was interrupted",
            )))
            .await
    })
}

fn wait_marker(reader: &mut BufReader<std::process::ChildStderr>, marker: &str) -> TestResult {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(format!("child closed stderr before {marker:?}").into());
        }
        if line.trim_end() == marker {
            return Ok(());
        }
    }
}

#[test]
fn interrupted_newline_free_stdout_exits_promptly_when_pipe_is_full() -> TestResult {
    if std::env::var_os(CHILD_ENV).is_some() {
        return run_child();
    }

    let (mut stdout_read, mut stdout_write) = std::io::pipe()?;
    let child_stdout = stdout_write.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child_stdin = child.stdin.take().ok_or("missing stdin")?;
    let mut stderr_lines = BufReader::new(child.stderr.take().ok_or("missing stderr")?);

    wait_marker(&mut stderr_lines, "STARTED")?;
    drain_nonblocking(&mut stdout_read)?;
    let _filled = fill_nonblocking(&mut stdout_write)?;
    child_stdin.write_all(b"1")?;
    drop(child_stdin);

    wait_marker(&mut stderr_lines, "READY")?;
    let pid = Pid::from_raw(i32::try_from(child.id())?).ok_or("no pid")?;
    kill_process(pid, Signal::INT)?;

    let exit_deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                "subprocess hung past 2s in exit cleanup after newline-free stdout write".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(stdout_write);
    drop(stdout_read);
    assert_eq!(status.code(), Some(1), "expected exit 1, got {status:?}");
    Ok(())
}
