//! Caller side of Proxy v1: one local byte stream bound to one remote TCP destination.

use std::{
    future::Future,
    io,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, ProxyResult, ServiceError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

use super::{
    ClientError, First, Sender, decoded, drained, empty_accept, first_or_cancel, next,
    sinks::settled, unexpected,
};
use crate::{FrameReader, FrameWriter};

/// Relays bytes between a local stream and the target's destination until the outcome.
///
/// Local EOF becomes `end`, and the destination's `end` shuts down the local write side, so
/// each direction closes independently and byte for byte. The two directions never wait on
/// each other. Each local write, flush, and shutdown is bounded by `idle` without progress so
/// a stalled local consumer cannot hold the relay indefinitely. Completing `cancel` sends
/// `cancel` and still reads the target's terminal outcome; a failed or timed-out local read or
/// write cancels the relay and reports that local error.
///
/// # Errors
/// Returns the refusal (`connection_refused`, `unreachable`, `dns_failed`, `timed_out`, or an
/// authorization failure), the stream failure, the failed local write or read, or the failed
/// outcome.
pub async fn run<R, W, LR, LW, C>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (local_read, local_write): (LR, LW),
    (idle, cancel): (Duration, C),
) -> Result<ProxyResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send,
    LR: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    tokio::pin!(cancel);
    let (answer, sender) = first_or_cancel((reader, writer), cancel.as_mut()).await?;
    match answer {
        First::Finished(frame) => return decoded(ProxyResult::decode_outcome(frame.payload())),
        First::Accepted(frame) => empty_accept(&frame)?,
    }
    let (lost, sent) = (Notify::new(), (AtomicU64::new(0), AtomicBool::new(false)));
    // A failed local read is kept even when the outcome arrives before the cancel is sent.
    let mut unread = None;
    let answer = {
        let downstream = downstream(reader, (local_write, idle), (&lost, &sent));
        tokio::pin!(downstream);
        // Once cancelled, nothing but the rest of that `cancel` is sent; the outcome still follows.
        match sender {
            Sender::Cancelled(rest) => drained(downstream.as_mut(), rest).await,
            Sender::Open(writer) => {
                let upstream = upstream(writer, (local_read, cancel), (&lost, &sent, &mut unread));
                tokio::pin!(upstream);
                tokio::select! {
                    answer = &mut downstream => answer,
                    () = &mut upstream => downstream.await,
                }
            }
        }
    };
    settled(answer, unread)
}

/// The bytes the caller sent toward the destination, and whether it sent its `end`.
type Sent = (AtomicU64, AtomicBool);

/// Local → destination, then `end` at local EOF, and a `cancel` once cancelled or the local
/// side is lost or fails; records what it sent in `sent`, and a failed local read in `unread`.
async fn upstream<W, LR, C>(
    writer: &mut FrameWriter<W>,
    (mut local_read, cancel): (LR, C),
    (lost, (sent, input_ended), unread): (&Notify, &Sent, &mut Option<std::io::Error>),
) where
    W: AsyncWrite + Unpin,
    LR: AsyncRead + Unpin,
    C: Future<Output = ()>,
{
    tokio::pin!(cancel);
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = tokio::select! {
            biased;
            () = lost.notified() => {
                let _cancelled = send_cancel(writer).await;
                return;
            }
            () = &mut cancel => {
                let _cancelled = send_cancel(writer).await;
                return;
            }
            read = local_read.read(&mut buffer) => read,
        };
        // A failed local read is a lost local side, not the end of its input.
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                *unread = Some(error);
                let _cancelled = send_cancel(writer).await;
                return;
            }
        };
        if read == 0 {
            let ended = async {
                writer.end(Channel::Input).await?;
                input_ended.store(true, Ordering::Relaxed);
                writer.flush().await
            }
            .await;
            if ended.is_ok() {
                // The destination may keep sending; a lost local side or a caller cancel still
                // cancels it.
                tokio::select! {
                    biased;
                    () = lost.notified() => {}
                    () = &mut cancel => {}
                }
                let _cancelled = send_cancel(writer).await;
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

/// Destination → local until the outcome; a lost local side asks upstream to cancel. Returns
/// the answer and the first local write that failed.
///
/// The destination's bytes end once, and the outcome counts exactly the bytes received from it
/// and at most the `sent` ones as taken to it: an answer with other counts is not this relay.
/// A relay completes only once both sides ended, with every byte sent taken to the
/// destination; any other end may have moved part of the traffic.
async fn downstream<R, LW>(
    reader: &mut FrameReader<R>,
    (mut local_write, idle): (LW, Duration),
    (lost, (sent, input_ended)): (&Notify, &Sent),
) -> (Result<ProxyResult, ClientError>, Option<io::Error>)
where
    R: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    let mut unwritten = None;
    let answer = async {
        let invalid = || ClientError::Stream(ServiceError::InvalidInput);
        let (mut ended, mut received) = (false, 0_u64);
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
                        match unwritten {
                            None => write_bounded(&mut local_write, bytes, idle).await,
                            Some(_) => Ok(()),
                        }
                    }
                    _ => return Err(invalid()),
                },
                FrameKind::End if !ended && frame.end() == Ok(Channel::Input) => {
                    ended = true;
                    match unwritten {
                        None => shutdown_bounded(&mut local_write, idle).await,
                        Some(_) => Ok(()),
                    }
                }
                FrameKind::Data | FrameKind::End => return Err(invalid()),
                _ => return Err(unexpected(&frame)),
            };
            if let Err(failure) = delivered {
                unwritten = Some(failure);
                lost.notify_one();
            }
        }
    }
    .await;
    (answer, unwritten)
}

/// Writes `bytes` and flushes `local_write`, allowing up to `idle` without progress on each
/// step so a slow consumer still draining receives every byte while a stalled one times out.
async fn write_bounded<LW: AsyncWrite + Unpin>(
    local_write: &mut LW,
    bytes: &[u8],
    idle: Duration,
) -> io::Result<()> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = tokio::time::timeout(idle, local_write.write(rest))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local write timed out"))??;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "local write returned zero bytes",
            ));
        }
        rest = rest.get(written..).unwrap_or_default();
    }
    tokio::time::timeout(idle, local_write.flush())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local flush timed out"))?
}

async fn shutdown_bounded<LW: AsyncWrite + Unpin>(
    local_write: &mut LW,
    idle: Duration,
) -> io::Result<()> {
    tokio::time::timeout(idle, local_write.shutdown())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local shutdown timed out"))?
}

async fn send_cancel<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
) -> Result<(), ServiceError> {
    writer.frame(FrameKind::Cancel, &[]).await?;
    writer.flush().await
}
