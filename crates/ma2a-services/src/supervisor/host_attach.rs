//! Attachments: many readers of retained and live output, and one stdin writer at a time.

use ma2a_core::service::{Channel, FrameKind, LogFrom, ServiceError, encode_skipped};
use tokio::io::AsyncWriteExt as _;

use super::{
    host::Shared,
    host_serve::{Reader, Writer},
    log,
    protocol::{ACKNOWLEDGED, encode_reading, encode_written},
};

/// Most content bytes read from the log for one batch of `data` frames.
const BATCH: usize = 262_144;

/// Holds the stdin writer lease for one connection and releases it on every exit path.
struct Lease<'a>(&'a Shared);

impl<'a> Lease<'a> {
    /// Takes the lease of a Process whose stdin is still open.
    async fn take(shared: &'a Shared) -> Result<Self, ServiceError> {
        if !shared.start.stdin() || shared.stdin.lock().await.is_none() {
            return Err(ServiceError::StdinClosed);
        }
        if shared.lease() {
            Ok(Self(shared))
        } else {
            Err(ServiceError::WriterBusy)
        }
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Follows output from `from`; with `stdin`, also relays channel 0 to the child.
pub(super) async fn attach(
    (reader, writer): (&mut Reader, &mut Writer),
    shared: &Shared,
    (from, stdin): (LogFrom, bool),
) -> Result<(), ServiceError> {
    let lease = if stdin {
        match Lease::take(shared).await {
            Ok(lease) => Some(lease),
            Err(error) => return writer.reject(error).await,
        }
    } else {
        None
    };
    let window = *shared.output.borrow();
    let start = match from {
        LogFrom::Start => window.first_available,
        LogFrom::Now => window.end,
        LogFrom::Cursor(cursor) if cursor > window.end => {
            return writer.reject(ServiceError::InvalidInput).await;
        }
        LogFrom::Cursor(cursor) => cursor.max(window.first_available),
    };
    let truncated = matches!(from, LogFrom::Cursor(cursor) if cursor < window.first_available);
    let accept = encode_reading((start, window.first_available, window.end), truncated);
    writer.frame(FrameKind::Accept, &accept).await?;
    let inbound = async {
        if lease.is_some() {
            relay_stdin(reader, shared).await.map(|_| ())
        } else {
            detached(reader).await
        }
    };
    tokio::select! {
        biased;
        // The caller detached; the child keeps running and the outcome reports where it was.
        result = inbound => {
            result?;
            let status = shared.status.borrow().to_bytes();
            writer.frame(FrameKind::Outcome, &status).await
        }
        result = follow(writer, shared, start) => result,
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
    let mut written = 0_u64;
    // A child that stops reading closes its stdin; the rest of the input is drained so the
    // daemon still reaches `end` and learns why.
    let mut failure = None;
    loop {
        let frame = reader.expect().await?;
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                if failure.is_none() {
                    match write(shared, bytes).await {
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
        shared.stdin.lock().await.is_none()
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

/// Relays channel 0 until the caller detaches; `end` closes the child's stdin for good.
async fn relay_stdin(reader: &mut Reader, shared: &Shared) -> Result<u64, ServiceError> {
    let mut written = 0_u64;
    loop {
        let Some(frame) = reader.next().await? else {
            return Ok(written);
        };
        match frame.kind() {
            FrameKind::Data => {
                let (Channel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                // Once the child closed its stdin, input is dropped; output keeps flowing.
                if write(shared, bytes).await.is_ok() {
                    written += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                }
            }
            FrameKind::End if frame.end()? == Channel::Input => {
                shared.stdin.lock().await.take();
            }
            FrameKind::Cancel => return Ok(written),
            _ => return Err(ServiceError::InvalidInput),
        }
    }
}

/// A reader without the lease may only detach.
async fn detached(reader: &mut Reader) -> Result<(), ServiceError> {
    match reader.next().await? {
        None => Ok(()),
        Some(frame) if frame.kind() == FrameKind::Cancel => Ok(()),
        Some(_) => Err(ServiceError::InvalidInput),
    }
}

/// Sends retained then live output from `cursor` until the child ended and all was sent.
async fn follow(writer: &mut Writer, shared: &Shared, mut cursor: u64) -> Result<(), ServiceError> {
    let mut output = shared.output.subscribe();
    let mut status = shared.status.subscribe();
    let directory = shared.log_directory();
    loop {
        // The final status is published only after every append, so reading it before the
        // window guarantees that a final status comes with the complete window.
        let ended = status.borrow_and_update().is_final();
        let window = *output.borrow_and_update();
        if cursor < window.first_available {
            cursor = window.first_available;
            let skipped = encode_skipped(cursor).map_err(|_| ServiceError::Internal)?;
            writer.frame(FrameKind::Control, &skipped).await?;
        }
        if cursor < window.end {
            let path = directory.clone();
            let read = tokio::task::spawn_blocking(move || log::read(&path, cursor, BATCH))
                .await
                .map_err(|_| ServiceError::Internal)?
                .map_err(|_| ServiceError::Io)?;
            if read.start > cursor {
                let skipped = encode_skipped(read.start).map_err(|_| ServiceError::Internal)?;
                writer.frame(FrameKind::Control, &skipped).await?;
            }
            for (channel, bytes) in &read.records {
                writer.data(*channel, bytes).await?;
            }
            cursor = read.next.max(read.start);
            if !read.records.is_empty() {
                continue;
            }
        }
        if ended {
            let last = status.borrow().to_bytes();
            return writer.frame(FrameKind::Outcome, &last).await;
        }
        tokio::select! {
            changed = output.changed() => changed.map_err(|_| ServiceError::Unavailable)?,
            changed = status.changed() => changed.map_err(|_| ServiceError::Unavailable)?,
        }
    }
}
