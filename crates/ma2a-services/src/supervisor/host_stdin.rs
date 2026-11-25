//! The stdin writer: one lease per Process, and writes the daemon can abandon.
//!
//! While a write waits for a child that does not read its stdin, the daemon's next frame is
//! still read, so its `cancel`, the end of its connection, or a failed read abandons the write
//! at once and frees the lease. The end of input is kept and reading goes on past it, since
//! only a `cancel` may follow, and any other frame is held. A write still waiting
//! [`INPUT_STALL`] after it began is abandoned as timed out, whatever follows it, so a child
//! that stops reading never holds the lease for longer.

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::io::{AsyncRead, AsyncWriteExt as _};

use super::{
    host::Shared,
    host_serve::{Reader, Writer},
    protocol::{ACKNOWLEDGED, encode_written},
    status::SupervisorStatus,
};
use crate::{Frame, FrameReader, input::INPUT_STALL};

/// Holds the stdin writer lease for one connection and releases it on every exit path.
pub(super) struct Lease<'a>(&'a Shared);

impl<'a> Lease<'a> {
    /// Takes the lease of a Process whose stdin is still open.
    ///
    /// A child not running, because it was not spawned yet, never started, or already ended,
    /// has no stdin to write: that is `NotRunning`, while a Process started without stdin, or
    /// whose stdin was ended, is `StdinClosed`.
    pub(super) async fn take(shared: &'a Shared) -> Result<Self, ServiceError> {
        if !shared.start.stdin() {
            return Err(ServiceError::StdinClosed);
        }
        if !matches!(*shared.status.borrow(), SupervisorStatus::Running(..)) {
            return Err(ServiceError::NotRunning);
        }
        // The lease is decided first: the stdin lock may be held by the current writer for as
        // long as the child does not read, and a second writer must not wait behind it.
        if !shared.lease() {
            return Err(ServiceError::WriterBusy);
        }
        let lease = Self(shared);
        if shared.stdin.lock().await.is_none() {
            return Err(ServiceError::StdinClosed);
        }
        Ok(lease)
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The daemon's frames, with the end of input and at most one more read ahead while a write
/// waited.
struct Daemon<'a, R> {
    reader: &'a mut FrameReader<R>,
    ended: Option<Frame>,
    ahead: Option<Frame>,
}

impl<'a, R: AsyncRead + Unpin> Daemon<'a, R> {
    const fn new(reader: &'a mut FrameReader<R>) -> Self {
        Self {
            reader,
            ended: None,
            ahead: None,
        }
    }

    /// The next frame: one read ahead, if any, or the next one read; `None` once the
    /// connection ended. Cancel-safe.
    async fn next(&mut self) -> Result<Option<Frame>, ServiceError> {
        if let Some(frame) = self.ended.take().or_else(|| self.ahead.take()) {
            return Ok(Some(frame));
        }
        self.reader.next().await
    }

    /// Runs one write to the child's stdin, returning how it went.
    ///
    /// # Errors
    /// Returns `Cancelled` when the daemon cancelled or closed its connection, `TimedOut` when
    /// the write still waited [`INPUT_STALL`] after it began, and the failed read otherwise;
    /// the write is abandoned.
    async fn write<F>(&mut self, write: F) -> Result<Result<(), ServiceError>, ServiceError>
    where
        F: Future<Output = Result<(), ServiceError>>,
    {
        tokio::pin!(write);
        let stall = tokio::time::sleep(INPUT_STALL);
        tokio::pin!(stall);
        loop {
            tokio::select! {
                biased;
                written = &mut write => return Ok(written),
                frame = self.reader.next(), if self.ahead.is_none() => match frame? {
                    Some(frame) if frame.kind() == FrameKind::Cancel => {
                        return Err(ServiceError::Cancelled);
                    }
                    Some(frame) if self.ended.is_none() && frame.end() == Ok(Channel::Input) => {
                        self.ended = Some(frame);
                    }
                    Some(frame) => self.ahead = Some(frame),
                    None => return Err(ServiceError::Cancelled),
                },
                () = &mut stall => return Err(ServiceError::TimedOut),
            }
        }
    }
}

/// Writes stdin until `end`; with `close`, the child's stdin is closed afterwards.
pub(super) async fn stdin(
    (reader, writer): (&mut Reader, &mut Writer),
    shared: &Shared,
    close: bool,
) -> Result<(), ServiceError> {
    let _lease = match Lease::take(shared).await {
        Ok(lease) => lease,
        Err(error) => return writer.reject(error).await,
    };
    writer.frame(FrameKind::Accept, ACKNOWLEDGED).await?;
    let mut daemon = Daemon::new(reader);
    let mut written = 0_u64;
    // A child that stops reading closes its stdin; the rest of the input is drained so the
    // daemon still reaches `end` and learns why.
    let mut failure = None;
    let mut changes = shared.status.subscribe();
    let mut child_ended = false;
    loop {
        let frame = tokio::select! {
            frame = daemon.next() => frame?.ok_or(ServiceError::Cancelled)?,
            // Nothing more can reach the stdin of a child that ended, so the write ends too.
            () = ended(&mut changes) => {
                child_ended = true;
                break;
            }
        };
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                if failure.is_none() {
                    // Also a write to a pipe that a process outside the child's group keeps.
                    let result = tokio::select! {
                        result = daemon.write(write(shared, bytes)) => result,
                        () = ended(&mut changes) => {
                            child_ended = true;
                            break;
                        }
                    };
                    let result = match result {
                        // The daemon gives up on the write too, and is told why if it still waits.
                        Err(ServiceError::TimedOut) => {
                            return writer.reject(ServiceError::TimedOut).await;
                        }
                        result => result?,
                    };
                    match result {
                        Ok(()) => written += u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                        Err(error) => failure = Some(error),
                    }
                }
            }
            FrameKind::End if frame.end()? == Channel::Input => break,
            _ => return Err(ServiceError::InvalidInput),
        }
    }
    if let Some(error) = failure {
        return writer.reject(error).await;
    }
    let closed = if close {
        shared.stdin.lock().await.take();
        true
    } else {
        child_ended || shared.stdin.lock().await.is_none()
    };
    writer
        .frame(FrameKind::Outcome, &encode_written(written, closed))
        .await
}

async fn write(shared: &Shared, bytes: &[u8]) -> Result<(), ServiceError> {
    let mut slot = shared.stdin.lock().await;
    let pipe = slot.as_mut().ok_or(ServiceError::StdinClosed)?;
    if pipe.write_all(bytes).await.is_err() {
        // The child no longer reads its stdin; it stays closed.
        slot.take();
        drop(slot);
        return Err(ServiceError::StdinClosed);
    }
    drop(slot);
    Ok(())
}

/// Relays channel 0 until the caller detaches, or until input the child did not take stalled,
/// which returns `true`; `end` closes the child's stdin for good.
pub(super) async fn relay_stdin<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    shared: &Shared,
) -> Result<bool, ServiceError> {
    let mut daemon = Daemon::new(reader);
    loop {
        let Some(frame) = daemon.next().await? else {
            return Ok(false);
        };
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                // Once the child closed its stdin, input is dropped; output keeps flowing.
                match daemon.write(write(shared, bytes)).await {
                    Ok(_) => {}
                    // The daemon detached during the write, or it stalled.
                    Err(ServiceError::Cancelled) => return Ok(false),
                    Err(ServiceError::TimedOut) => return Ok(true),
                    Err(error) => return Err(error),
                }
            }
            FrameKind::End if frame.end()? == Channel::Input => {
                shared.stdin.lock().await.take();
            }
            FrameKind::Cancel => return Ok(false),
            _ => return Err(ServiceError::InvalidInput),
        }
    }
}

/// Completes once the child has ended; never, while its status cannot change any more.
async fn ended(changes: &mut tokio::sync::watch::Receiver<SupervisorStatus>) {
    while !changes.borrow_and_update().is_final() {
        if changes.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
#[path = "host_stdin_tests.rs"]
mod tests;
