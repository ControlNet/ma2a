//! Attachments: many readers of retained and live output, and one stdin writer at a time.

use ma2a_core::service::{FrameKind, LogFrom, ServiceError, encode_skipped};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

use super::{
    host::Shared,
    host_stdin::{Lease, relay_stdin},
    log,
    protocol::encode_reading,
    status::SupervisorStatus,
};
use crate::{FrameReader, FrameWriter};

/// Most content bytes read from the log for one batch of `data` frames.
const BATCH: usize = 262_144;

/// Follows output from `from`; with `stdin`, also relays channel 0 to the child.
pub(super) async fn attach<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
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
    let Some((start, truncated)) = window.resolve(from) else {
        return writer.reject(ServiceError::InvalidInput).await;
    };
    let accept = encode_reading((start, window.first_available, window.end), truncated);
    writer.frame(FrameKind::Accept, &accept).await?;
    let inbound = async {
        if lease.is_some() {
            relay_stdin(reader, shared).await
        } else {
            detached(reader).await.map(|()| false)
        }
    };
    // The status a detach reports, taken when it was decided.
    let (detach, detached) = watch::channel(None);
    let following = follow(writer, shared, (start, detached));
    tokio::pin!(following);
    tokio::select! {
        // The caller detached, or its input stalled; the child keeps running. The frame being
        // written is finished first, so the outcome that reports where the child was follows
        // whole frames.
        result = inbound => {
            let stalled = result?;
            let now = *shared.status.borrow();
            // The daemon reads a final status after a detach it did not ask for as the
            // child's end, so a child that already ended has all its output sent instead.
            if !(stalled && now.is_final()) {
                detach.send_replace(Some(now));
            }
            following.await
        }
        result = &mut following => result,
    }
}

/// A reader without the lease may only detach.
async fn detached<R: AsyncRead + Unpin>(reader: &mut FrameReader<R>) -> Result<(), ServiceError> {
    match reader.next().await? {
        None => Ok(()),
        Some(frame) if frame.kind() == FrameKind::Cancel => Ok(()),
        Some(_) => Err(ServiceError::InvalidInput),
    }
}

/// Sends retained then live output from `cursor` until the child ended and all was sent, with
/// its status as the outcome, or until a detach, checked between frames, with the status it
/// carries.
async fn follow<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    shared: &Shared,
    (mut cursor, mut detach): (u64, watch::Receiver<Option<SupervisorStatus>>),
) -> Result<(), ServiceError> {
    let mut output = shared.output.subscribe();
    let mut status = shared.status.subscribe();
    let directory = shared.log_directory();
    loop {
        let detached = *detach.borrow();
        if let Some(now) = detached {
            return writer.frame(FrameKind::Outcome, &now.to_bytes()).await;
        }
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
                if detach.borrow().is_some() {
                    break;
                }
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
            changed = detach.changed() => changed.map_err(|_| ServiceError::Internal)?,
        }
    }
}

#[cfg(all(test, unix))]
#[path = "host_attach_tests.rs"]
mod tests;
