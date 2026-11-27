//! Client → target half of the stream-plane relay.
//!
//! While a frame waits for room toward the target (which can be up to a Proxy's idle timeout),
//! the client's stream is still watched with one-frame lookahead so a client cancel, loss, or
//! protocol violation caps the remaining wait at [`CANCEL_GRACE`] instead of holding the stream
//! for the full stall bound.

use std::{pin::Pin, sync::atomic::Ordering, time::Duration};

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Instant, Sleep},
};

use super::{CANCEL_GRACE, Shared, forward, from_client};

/// What arrived from the client while the previous frame was still waiting for target room.
enum Queued {
    /// A `cancel` frame from the client: forward it if the current write finishes in time.
    Cancel,
    /// Client disconnected, read failed, or sent a disallowed frame; `gone` says whether the
    /// client can still hear the terminal.
    Stop { gone: bool },
}

/// Frames read ahead from the client while a write toward the target is waiting for room.
#[derive(Default)]
struct Lookahead {
    ended: Option<Frame>,
    ahead: Option<Frame>,
    queued: Option<Queued>,
}

/// Client → target until the client cancels, breaks the contract, or disappears; each of
/// those leaves the target cancelled. Returning starts the cancel grace, unless it returns
/// `false`: a write toward the target ran out of its bound, which already was the wait.
pub(super) async fn run<CR, TW>(
    (client_reader, target_writer): (&mut FrameReader<CR>, &mut FrameWriter<TW>),
    (shared, stall): (&Shared, Duration),
) -> bool
where
    CR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
{
    let mut lookahead = Lookahead::default();
    loop {
        if let Some(stop) = lookahead.queued.take() {
            return finish_stop(target_writer, shared, stop).await;
        }
        if shared.client_gone.load(Ordering::Relaxed) {
            return finish_stop(target_writer, shared, Queued::Stop { gone: true }).await;
        }
        let next = if let Some(frame) = lookahead.ended.take().or_else(|| lookahead.ahead.take()) {
            Some(Ok(Some(frame)))
        } else {
            tokio::select! {
                biased;
                () = shared.client_lost.notified() => None,
                frame = client_reader.next() => Some(frame),
            }
        };
        match next {
            Some(Ok(Some(frame))) if from_client(frame.kind()) => {
                Shared::count(&shared.to_target, &frame);
                if frame.kind() == FrameKind::Cancel {
                    // A cancel gets the grace a cancel for a client that left gets.
                    return match forward(target_writer, &frame, CANCEL_GRACE).await {
                        Err(ServiceError::TimedOut) => false,
                        // A target that stopped reading may have ended; its terminal can follow.
                        Ok(()) | Err(_) => true,
                    };
                }
                match send_watching(
                    (client_reader, target_writer),
                    (&frame, stall),
                    (shared, &mut lookahead),
                )
                .await
                {
                    Ok(()) => {}
                    Err(ServiceError::TimedOut) => return false,
                    // A target that stopped reading may have ended; its terminal can follow.
                    Err(_) => return true,
                }
            }
            other => {
                // A protocol violation leaves the client able to hear the terminal; a closed
                // connection, a read failure, or a lost client does not.
                let gone = !matches!(other, Some(Ok(Some(_))));
                return finish_stop(target_writer, shared, Queued::Stop { gone }).await;
            }
        }
    }
}

/// Sends `frame` toward the target within `bound`, watching the client so a cancel or loss
/// shortens the remaining wait to [`CANCEL_GRACE`].
async fn send_watching<CR, TW>(
    (client_reader, target_writer): (&mut FrameReader<CR>, &mut FrameWriter<TW>),
    (frame, bound): (&Frame, Duration),
    (shared, lookahead): (&Shared, &mut Lookahead),
) -> Result<(), ServiceError>
where
    CR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
{
    let sent = async {
        target_writer.frame(frame.kind(), frame.payload()).await?;
        target_writer.flush().await
    };
    let deadline = tokio::time::sleep(bound);
    tokio::pin!(sent, deadline);
    let mut shortened = false;
    loop {
        let watch_client = lookahead.ahead.is_none() && lookahead.queued.is_none();
        tokio::select! {
            biased;
            forwarded = &mut sent => return forwarded,
            () = shared.client_lost.notified(), if !shortened => {
                shared.client_gone.store(true, Ordering::Relaxed);
                lookahead.queued = Some(Queued::Stop { gone: true });
                shorten(&mut deadline, &mut shortened);
            }
            next = client_reader.next(), if watch_client => match next {
                Ok(Some(frame)) if frame.kind() == FrameKind::Cancel => {
                    lookahead.queued = Some(Queued::Cancel);
                    shorten(&mut deadline, &mut shortened);
                }
                Ok(Some(frame))
                    if lookahead.ended.is_none() && frame.end() == Ok(Channel::Input) =>
                {
                    lookahead.ended = Some(frame);
                }
                Ok(Some(frame)) if from_client(frame.kind()) => {
                    lookahead.ahead = Some(frame);
                }
                other => {
                    let gone = !matches!(other, Ok(Some(_)));
                    if gone {
                        shared.client_gone.store(true, Ordering::Relaxed);
                    }
                    lookahead.queued = Some(Queued::Stop { gone });
                    shorten(&mut deadline, &mut shortened);
                }
            },
            () = &mut deadline => return Err(ServiceError::TimedOut),
        }
    }
}

fn shorten(deadline: &mut Pin<&mut Sleep>, shortened: &mut bool) {
    if !*shortened {
        *shortened = true;
        let soon = Instant::now() + CANCEL_GRACE;
        if soon < deadline.deadline() {
            deadline.as_mut().reset(soon);
        }
    }
}

async fn finish_stop<TW: AsyncWrite + Unpin>(
    target_writer: &mut FrameWriter<TW>,
    shared: &Shared,
    stop: Queued,
) -> bool {
    if matches!(stop, Queued::Stop { gone: true }) {
        shared.client_gone.store(true, Ordering::Relaxed);
    }
    let cancel = async {
        target_writer.frame(FrameKind::Cancel, &[]).await?;
        target_writer.flush().await
    };
    tokio::time::timeout(CANCEL_GRACE, cancel).await.is_ok()
}
