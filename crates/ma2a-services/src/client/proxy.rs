//! Caller side of Proxy v1: one local byte stream bound to one remote TCP destination.

mod downstream;

use std::{
    future::Future,
    io,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use downstream::downstream;
use ma2a_core::service::{Channel, EndReason, MAX_DATA_CHUNK_LEN, ProxyResult, ServiceError};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use super::{
    ClientError, First, Sender, Written, decoded, drained, empty_accept, first_or_cancel,
    send_cancel, sinks::settled, write_watching,
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
        Sent {
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            ended: AtomicBool::new(false),
        },
    );
    // A failed local read is kept even when the outcome arrives before the cancel is sent.
    let mut unread = None;
    let (answer, unwritten, delivery_interrupted) = {
        let mut downstream = Box::pin(downstream(
            reader,
            (local_write, idle),
            (&lost, &stop, &sent),
        ));
        // Once cancelled, nothing but the rest of that `cancel` is sent; the outcome still follows.
        match sender {
            Sender::Cancelled(rest) => {
                stop.cancel();
                drained(downstream.as_mut(), rest).await.unwrap_or((
                    Err(ClientError::Stream(ServiceError::OutcomeUnknown)),
                    None,
                    false,
                ))
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
                            .unwrap_or((Err(ClientError::Stream(ServiceError::OutcomeUnknown)), None, false))
                    } else {
                        downstream.await
                    },
                }
            }
        }
    };
    let result = settled((answer, unwritten), unread)?;
    if delivery_interrupted && result.reason == EndReason::Completed {
        return Err(ClientError::Local(io::Error::new(
            io::ErrorKind::Interrupted,
            "local output delivery was interrupted",
        )));
    }
    Ok(result)
}

/// Bytes started and completed toward the destination, and whether `end` started.
pub(super) struct Sent {
    pub(super) started: AtomicU64,
    pub(super) completed: AtomicU64,
    pub(super) ended: AtomicBool,
}

/// Local → destination, then `end` at local EOF, and a `cancel` once cancelled or the local
/// side is lost or fails; records what it sent in `sent`, and a failed local read in `unread`.
async fn upstream<W, LR, C>(
    writer: &mut FrameWriter<W>,
    (mut local_read, cancel): (LR, C),
    (lost, stop, sent, unread): (
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
                sent.ended.store(true, Ordering::Release);
                writer.end(Channel::Input).await?;
                writer.flush().await
            };
            match write_watching(ended, (lost, cancel.as_mut(), stop)).await {
                Written::Done(Ok(())) => {
                    // The destination may keep sending; a lost local side or a caller cancel still
                    // cancels it.
                    wait_stop(lost, cancel.as_mut(), stop).await;
                    send_cancel(writer).await;
                }
                Written::Done(Err(_)) => wait_stop(lost, cancel.as_mut(), stop).await,
                Written::Cancelled { finished: false } => {}
                Written::Cancelled { finished: true } => send_cancel(writer).await,
            }
            return;
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        let len = u64::try_from(read).unwrap_or(u64::MAX);
        let forwarded = async {
            let total = sent
                .started
                .fetch_add(len, Ordering::AcqRel)
                .saturating_add(len);
            writer.data(Channel::Input, chunk).await?;
            sent.completed.store(total, Ordering::Release);
            writer.flush().await
        };
        match write_watching(forwarded, (lost, cancel.as_mut(), stop)).await {
            Written::Done(Ok(())) => {}
            Written::Done(Err(_)) => {
                wait_stop(lost, cancel.as_mut(), stop).await;
                return;
            }
            Written::Cancelled { finished: false } => return,
            Written::Cancelled { finished: true } => {
                send_cancel(writer).await;
                return;
            }
        }
    }
}

/// Waits until `lost` or `cancel` fires, then cancels `stop` so `downstream` bounds its wait.
async fn wait_stop<C: Future<Output = ()>>(
    lost: &Notify,
    cancel: std::pin::Pin<&mut C>,
    stop: &CancellationToken,
) {
    tokio::select! {
        biased;
        () = lost.notified() => {}
        () = cancel => {}
    }
    stop.cancel();
}
