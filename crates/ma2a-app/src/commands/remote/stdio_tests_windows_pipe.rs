//! Windows subprocess regression test for [`StdioGuard`] full-pipe exit cleanup.

use std::{
    error::Error,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::io::AsyncWriteExt as _;

use super::super::{StdioGuard, interrupted_io};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const PIPE_EXIT_CHILD_ENV: &str = "MA2A_TEST_STDIO_WIN_PIPE_EXIT_CHILD";
const PIPE_EXIT_TEST_NAME: &str = "commands::remote::stdio::tests::pipe::interrupted_newline_free_stdout_exits_promptly_when_pipe_is_full";

fn run_pipe_exit_child() -> TestResult {
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

        let stdio = StdioGuard::default();
        let mut stdout = stdio.stdout();
        stdout.write_all(b"{\"ok\":true}").await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        stdio.exit_error(interrupted_io().into()).await
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
    if std::env::var_os(PIPE_EXIT_CHILD_ENV).is_some() {
        return run_pipe_exit_child();
    }

    let (stdout_read, mut stdout_write) = std::io::pipe()?;
    let child_stdout = stdout_write.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", PIPE_EXIT_TEST_NAME, "--nocapture"])
        .env(PIPE_EXIT_CHILD_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child_stdin = child.stdin.take().ok_or("missing stdin")?;
    let mut stderr_lines = BufReader::new(child.stderr.take().ok_or("missing stderr")?);
    wait_marker(&mut stderr_lines, "STARTED")?;

    let writing = Arc::new(AtomicBool::new(false));
    let writing_flag = Arc::clone(&writing);
    let filler = std::thread::spawn(move || {
        let chunk = [0_u8; 4_096];
        loop {
            writing_flag.store(true, Ordering::Release);
            if stdout_write.write_all(&chunk).is_err() {
                break;
            }
            writing_flag.store(false, Ordering::Release);
        }
    });
    let fill_deadline = Instant::now() + Duration::from_secs(2);
    let mut blocked_since = None;
    while Instant::now() < fill_deadline {
        if writing.load(Ordering::Acquire) {
            let since = *blocked_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(50) {
                break;
            }
        } else {
            blocked_since = None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child_stdin.write_all(b"1")?;
    drop(child_stdin);

    let exit_deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let _ = child.kill();
            let _ = child.wait();
            drop(stdout_read);
            let _ = filler.join();
            return Err(
                "subprocess hung past 2s in exit cleanup after newline-free stdout write".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(stdout_read);
    let _ = filler.join();
    assert_eq!(status.code(), Some(1), "expected exit 1, got {status:?}");
    Ok(())
}
