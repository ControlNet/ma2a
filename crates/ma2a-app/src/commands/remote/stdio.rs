//! Observed local `stdout` / `stderr` sinks and bounded exit helpers.
//!
//! When a streaming CLI command is interrupted or times out while `tokio::io::stdout()` or
//! `tokio::io::stderr()` is blocked in a `spawn_blocking` write on a full pipe, that worker
//! remains stuck in the kernel `write` syscall holding the standard stream's lock. Returning to
//! `#[tokio::main]` would hang forever in `Runtime::drop`, and synchronously locking or flushing
//! the blocked stream would deadlock before exiting.

use std::{
    io::{self, Write as _},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use tokio::io::{AsyncWrite, AsyncWriteExt as _};

use crate::AppError;

/// Upper bound on writing an exit diagnostic to stderr when stderr itself may be a full pipe.
pub(super) const DIAGNOSTIC_GRACE: Duration = Duration::from_millis(100);

/// Tracks whether `tokio::io::stdout()` or `tokio::io::stderr()` has a blocking worker in flight.
#[derive(Clone, Debug, Default)]
pub(super) struct StdioGuard {
    stdout_stuck: Arc<AtomicBool>,
    stderr_stuck: Arc<AtomicBool>,
}

impl StdioGuard {
    pub(super) fn stdout(&self) -> TrackedWrite<tokio::io::Stdout> {
        TrackedWrite {
            inner: tokio::io::stdout(),
            in_flight: Arc::clone(&self.stdout_stuck),
        }
    }

    pub(super) fn stderr(&self) -> TrackedWrite<tokio::io::Stderr> {
        TrackedWrite {
            inner: tokio::io::stderr(),
            in_flight: Arc::clone(&self.stderr_stuck),
        }
    }

    pub(super) fn stdout_stuck(&self) -> bool {
        self.stdout_stuck.load(Ordering::Acquire)
    }

    pub(super) fn stderr_stuck(&self) -> bool {
        self.stderr_stuck.load(Ordering::Acquire)
    }

    pub(super) fn any_stuck(&self) -> bool {
        self.stdout_stuck() || self.stderr_stuck()
    }

    /// Writes one diagnostic line to stderr without blocking indefinitely if stderr is full or
    /// its blocking worker is stuck.
    pub(super) async fn note_stderr(&self, line: &str) {
        if self.stderr_stuck() {
            return;
        }
        let mut bytes = Vec::with_capacity(line.len().saturating_add(1));
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let _written = tokio::time::timeout(DIAGNOSTIC_GRACE, async {
            let mut stderr = self.stderr();
            if stderr.write_all(&bytes).await.is_ok() {
                let _ = stderr.flush().await;
            }
        })
        .await;
    }

    /// Exits directly with `code`, flushing stdout and stderr only when `flush_outputs` is true
    /// and neither stream has a stuck blocking worker.
    pub(super) async fn exit_code(&self, code: i32, flush: bool) -> ! {
        if flush
            && !self.any_stuck()
            && let Err(error) = flush_outputs()
        {
            self.note_stderr(&AppError::Io(error).to_string()).await;
            std::process::exit(1);
        }
        std::process::exit(code)
    }

    /// Reports `error` to stderr if unblocked and exits with `error.exit_code()`.
    pub(super) async fn exit_error(&self, error: AppError) -> ! {
        self.note_stderr(&error.to_string()).await;
        std::process::exit(i32::from(error.exit_code()))
    }
}

fn flush_outputs() -> io::Result<()> {
    io::stdout().lock().flush()?;
    io::stderr().lock().flush()
}

/// [`AsyncWrite`] wrapper that leaves `in_flight` true whenever dropped while a write or flush
/// is still pending in `spawn_blocking`.
#[derive(Debug)]
pub(super) struct TrackedWrite<W> {
    inner: W,
    in_flight: Arc<AtomicBool>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TrackedWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.in_flight.store(true, Ordering::Release);
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if poll.is_ready() {
            self.in_flight.store(false, Ordering::Release);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.in_flight.store(true, Ordering::Release);
        let poll = Pin::new(&mut self.inner).poll_flush(cx);
        if poll.is_ready() {
            self.in_flight.store(false, Ordering::Release);
        }
        poll
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.in_flight.store(true, Ordering::Release);
        let poll = Pin::new(&mut self.inner).poll_shutdown(cx);
        if poll.is_ready() {
            self.in_flight.store(false, Ordering::Release);
        }
        poll
    }
}
