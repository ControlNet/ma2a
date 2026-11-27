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
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use super::{
    ClientError, First, Sender, Written, decoded, drained, empty_accept, first_or_cancel, next,
    send_cancel,
    sinks::{settled, shutdown_bounded, write_bounded, write_until},
    unexpected, write_watching,
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
    let (lost, stop, sent) = (
        Notify::new(),
        CancellationToken::new(),
        (AtomicU64::new(0), AtomicBool::new(false)),
    );
    // A failed local read is kept even when the outcome arrives before the cancel is sent.
    let mut unread = None;
    let answer = {
        let mut downstream = Box::pin(downstream(
            reader,
            (local_write, idle),
            (&lost, &stop, &sent),
        ));
        // Once cancelled, nothing but the rest of that `cancel` is sent; the outcome still follows.
        match sender {
            Sender::Cancelled(rest) => {
                stop.cancel();
                drained(downstream.as_mut(), rest)
                    .await
                    .unwrap_or((Err(ClientError::Stream(ServiceError::OutcomeUnknown)), None))
            }
            Sender::Open(writer) => {
                let mut upstream = Box::pin(upstream(
                    writer,
                    (local_read, cancel),
                    (&lost, &stop, &sent, &mut unread),
                ));
                tokio::select! {
                    answer = &mut downstream => answer,
                    () = &mut upstream => if stop.is_cancelled() {
                        drained(downstream.as_mut(), None)
                            .await
                            .unwrap_or((Err(ClientError::Stream(ServiceError::OutcomeUnknown)), None))
                    } else {
                        downstream.await
                    },
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
    (lost, stop, (sent, input_ended), unread): (
        &Notify,
        &CancellationToken,
        &Sent,
        &mut Option<std::io::Error>,
    ),
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
                stop.cancel();
                send_cancel(writer).await;
                return;
            }
            () = &mut cancel => {
                stop.cancel();
                send_cancel(writer).await;
                return;
            }
            read = local_read.read(&mut buffer) => read,
        };
        // A failed local read is a lost local side, not the end of its input.
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                *unread = Some(error);
                stop.cancel();
                send_cancel(writer).await;
                return;
            }
        };
        if read == 0 {
            let ended = async {
                writer.end(Channel::Input).await?;
                input_ended.store(true, Ordering::Relaxed);
                writer.flush().await
            };
            match write_watching(ended, (lost, cancel.as_mut(), stop)).await {
                Written::Done(Ok(())) => {
                    // The destination may keep sending; a lost local side or a caller cancel still
                    // cancels it.
                    tokio::select! {
                        biased;
                        () = lost.notified() => {}
                        () = &mut cancel => {}
                    }
                    stop.cancel();
                    send_cancel(writer).await;
                }
                Written::Done(Err(_)) | Written::Cancelled { finished: false } => {}
                Written::Cancelled { finished: true } => send_cancel(writer).await,
            }
            return;
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        let forwarded = async {
            writer.data(Channel::Input, chunk).await?;
            sent.fetch_add(u64::try_from(read).unwrap_or(u64::MAX), Ordering::Relaxed);
            writer.flush().await
        };
        match write_watching(forwarded, (lost, cancel.as_mut(), stop)).await {
            Written::Done(Ok(())) => {}
            Written::Done(Err(_)) | Written::Cancelled { finished: false } => return,
            Written::Cancelled { finished: true } => {
                send_cancel(writer).await;
                return;
            }
        }
    }
}

/// Destination → local until the outcome; a lost local side asks upstream to cancel, and
/// explicit cancellation interrupts local delivery and discards remaining output while still
/// checking frames and byte counts. Returns the answer and the first local write that failed.
///
/// The destination's bytes end once, and the outcome counts exactly the bytes received from it
/// and at most the `sent` ones as taken to it: an answer with other counts is not this relay.
/// A relay completes only once both sides ended, with every byte sent taken to the
/// destination; any other end may have moved part of the traffic.
async fn downstream<R, LW>(
    reader: &mut FrameReader<R>,
    (mut local_write, idle): (LW, Duration),
    (lost, stop, (sent, input_ended)): (&Notify, &CancellationToken, &Sent),
) -> (Result<ProxyResult, ClientError>, Option<io::Error>)
where
    R: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    let mut unwritten = None;
    let mut discarding = false;
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
                        if unwritten.is_some() {
                            Some(Ok(()))
                        } else if discarding || stop.is_cancelled() {
                            None
                        } else {
                            write_until(write_bounded(&mut local_write, bytes, idle), stop).await
                        }
                    }
                    _ => return Err(invalid()),
                },
                FrameKind::End if !ended && frame.end() == Ok(Channel::Input) => {
                    ended = true;
                    if unwritten.is_some() {
                        Some(Ok(()))
                    } else if discarding || stop.is_cancelled() {
                        None
                    } else {
                        write_until(shutdown_bounded(&mut local_write, idle), stop).await
                    }
                }
                FrameKind::Data | FrameKind::End => return Err(invalid()),
                _ => return Err(unexpected(&frame)),
            };
            match delivered {
                Some(Ok(())) => {}
                Some(Err(failure)) => {
                    unwritten = Some(failure);
                    lost.notify_one();
                }
                None => discarding = true,
            }
        }
    }
    .await;
    (answer, unwritten)
}
