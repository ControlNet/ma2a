//! Attachments: many readers of retained and live output, and one stdin writer at a time.

use ma2a_core::service::{FrameKind, LogFrom, ServiceError, encode_skipped};
use tokio_util::sync::CancellationToken;

use super::{
    host::Shared,
    host_serve::{Reader, Writer},
    host_stdin::{Lease, relay_stdin},
    log,
    protocol::encode_reading,
};

/// Most content bytes read from the log for one batch of `data` frames.
const BATCH: usize = 262_144;

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
    let Some((start, truncated)) = window.resolve(from) else {
        return writer.reject(ServiceError::InvalidInput).await;
    };
    let accept = encode_reading((start, window.first_available, window.end), truncated);
    writer.frame(FrameKind::Accept, &accept).await?;
    let inbound = async {
        if lease.is_some() {
            relay_stdin(reader, shared).await.map(|_| ())
        } else {
            detached(reader).await
        }
    };
    let detach = CancellationToken::new();
    let following = follow(writer, shared, (start, &detach));
    tokio::pin!(following);
    tokio::select! {
        biased;
        // The caller detached; the child keeps running. The frame being written is finished
        // first, so the outcome that reports where the child was follows whole frames.
        result = inbound => {
            result?;
            detach.cancel();
            following.await
        }
        result = &mut following => result,
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

/// Sends retained then live output from `cursor` until the child ended and all was sent, or
/// until `detach`, checked between frames; either way the outcome is the child's status.
async fn follow(
    writer: &mut Writer,
    shared: &Shared,
    (mut cursor, detach): (u64, &CancellationToken),
) -> Result<(), ServiceError> {
    let mut output = shared.output.subscribe();
    let mut status = shared.status.subscribe();
    let directory = shared.log_directory();
    loop {
        if detach.is_cancelled() {
            let now = shared.status.borrow().to_bytes();
            return writer.frame(FrameKind::Outcome, &now).await;
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
                if detach.is_cancelled() {
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
            () = detach.cancelled() => {}
        }
    }
}
