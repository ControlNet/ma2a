//! Caller side of Proxy v1: one local byte stream bound to one remote TCP destination.

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, ProxyResult, ServiceError,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

use super::{
    ClientError, First, decoded, empty_accept, first, next, sinks::write_flushed, unexpected,
};
use crate::{FrameReader, FrameWriter};

/// Relays bytes between a local stream and the target's destination until the outcome.
///
/// Local EOF becomes `end`, and the destination's `end` shuts down the local write side, so
/// each direction closes independently and byte for byte. The two directions never wait on
/// each other.
///
/// # Errors
/// Returns the refusal (`connection_refused`, `unreachable`, `dns_failed`, `timed_out`, or an
/// authorization failure), the failed outcome, or the stream failure.
pub async fn run<R, W, LR, LW>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (local_read, local_write): (LR, LW),
) -> Result<ProxyResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    LR: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    match first(reader).await? {
        First::Finished(frame) => return decoded(ProxyResult::decode_outcome(frame.payload())),
        First::Accepted(frame) => empty_accept(&frame)?,
    }
    let (lost, sent) = (Notify::new(), (AtomicU64::new(0), AtomicBool::new(false)));
    let downstream = downstream(reader, local_write, (&lost, &sent));
    let upstream = upstream(writer, local_read, (&lost, &sent));
    tokio::pin!(downstream, upstream);
    tokio::select! {
        biased;
        result = &mut downstream => result,
        () = &mut upstream => downstream.await,
    }
}

/// The bytes the caller sent toward the destination, and whether it sent its `end`.
type Sent = (AtomicU64, AtomicBool);

/// Local → destination, then `end` at local EOF, and a `cancel` once the local side is lost or
/// fails; records what it sent in `sent`.
async fn upstream<W, LR>(
    writer: &mut FrameWriter<W>,
    mut local_read: LR,
    (lost, (sent, input_ended)): (&Notify, &Sent),
) where
    W: AsyncWrite + Unpin,
    LR: AsyncRead + Unpin,
{
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = tokio::select! {
            biased;
            () = lost.notified() => {
                let _cancelled = cancel(writer).await;
                return;
            }
            read = local_read.read(&mut buffer) => read,
        };
        // A failed local read is a lost local side, not the end of its input.
        let Ok(read) = read else {
            let _cancelled = cancel(writer).await;
            return;
        };
        if read == 0 {
            let ended = async {
                writer.end(Channel::Input).await?;
                input_ended.store(true, Ordering::Relaxed);
                writer.flush().await
            }
            .await;
            if ended.is_ok() {
                // The destination may keep sending; a lost local side still cancels it.
                lost.notified().await;
                let _cancelled = cancel(writer).await;
            }
            return;
        }
        let forwarded = writer
            .data(Channel::Input, buffer.get(..read).unwrap_or_default())
            .await;
        if forwarded.is_ok() {
            sent.fetch_add(u64::try_from(read).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        if forwarded.and(writer.flush().await).is_err() {
            return;
        }
    }
}

/// Destination → local until the outcome; a lost local side asks upstream to cancel.
///
/// The destination's bytes end once, and the outcome counts exactly the bytes received from it
/// and at most the `sent` ones as taken to it: an answer with other counts is not this relay.
/// A relay completes only once both sides ended, with every byte sent taken to the
/// destination; any other end may have moved part of the traffic.
async fn downstream<R, LW>(
    reader: &mut FrameReader<R>,
    mut local_write: LW,
    (lost, (sent, input_ended)): (&Notify, &Sent),
) -> Result<ProxyResult, ClientError>
where
    R: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    let invalid = || ClientError::Stream(ServiceError::InvalidInput);
    let (mut delivering, mut ended, mut received) = (true, false, 0_u64);
    loop {
        let frame = next(reader).await?;
        let delivered = match frame.kind() {
            FrameKind::Outcome => {
                let result = decoded(ProxyResult::decode_outcome(frame.payload()))?;
                let (sent, completed) = (
                    sent.load(Ordering::Relaxed),
                    result.reason == EndReason::Completed,
                );
                if completed && !(ended && input_ended.load(Ordering::Relaxed)) {
                    return Err(invalid());
                }
                let taken = if completed {
                    result.bytes_to_target == sent
                } else {
                    result.bytes_to_target <= sent
                };
                let counted = result.bytes_from_target == received && taken;
                if !counted {
                    return Err(ClientError::Stream(ServiceError::IntegrityMismatch));
                }
                return Ok(result);
            }
            FrameKind::Data if !ended => match frame.data().map_err(ClientError::Stream)? {
                (Channel::Input, bytes) => {
                    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    received = received.saturating_add(length);
                    !delivering || write_flushed(&mut local_write, bytes).await.is_ok()
                }
                _ => return Err(invalid()),
            },
            FrameKind::End if !ended && frame.end() == Ok(Channel::Input) => {
                ended = true;
                !delivering || local_write.shutdown().await.is_ok()
            }
            FrameKind::Data | FrameKind::End => return Err(invalid()),
            _ => return Err(unexpected(&frame)),
        };
        if !delivered {
            delivering = false;
            lost.notify_one();
        }
    }
}

async fn cancel<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>) -> Result<(), ServiceError> {
    writer.frame(FrameKind::Cancel, &[]).await?;
    writer.flush().await
}
