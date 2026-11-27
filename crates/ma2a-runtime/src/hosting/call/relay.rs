//! Relaying one delivered call between its caller and its service until a terminal.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use ma2a_core::service::{Channel, FrameKind};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream};
use tokio_util::sync::CancellationToken;

use super::super::{CallIo, fault};
use crate::stream_plane::relay::{CANCEL_GRACE, ClientWrite, FORWARD_STALL, Relayed, relay};

/// Bytes buffered between the caller's frames and the relay.
const PUMP_BUFFER: usize = 128 * 1024;
/// How long a call may take to end once it is being cancelled, even if its service stopped
/// reading: one second past the relay's own grace, so the relay ends every other call first.
const STUCK_GRACE: Duration = CANCEL_GRACE.saturating_add(Duration::from_secs(1));
/// How long one caller frame may wait for the service to take it. A caller's `cancel` queued
/// behind input the service never reads would otherwise never be seen, so a service that
/// takes no input for this long is treated as a cancelled call.
pub(crate) const INPUT_STALL: Duration = Duration::from_secs(30);

/// What the pump learned about how the call is ending.
#[derive(Default)]
struct Ending {
    /// Cancelled once the caller cancelled or left, or the call was stopped.
    cancelling: CancellationToken,
    /// Whether the caller left.
    left: AtomicBool,
}

/// Relays one delivered call; `stop` is the operation's and `service_stop` the service's.
///
/// A service that stops reading its call fills its connection, and the relay then waits
/// forever to forward the `cancel`. So the call itself also bounds how long it may take to end
/// once it is being cancelled; that end is lost, so the caller learns `outcome_unknown`, and its
/// byte counts are not known.
///
/// Both parties are held to the Services v1 order and direction rules: caller data or an `end`
/// on another channel than input, or after its `end` of input, cancels the call, and a service
/// frame out of order, on the caller's channel, or on a channel the service ended loses it.
pub(super) async fn relay_call<R, W>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    stream: Box<dyn CallIo>,
    (stop, service_stop): (&CancellationToken, &CancellationToken),
) -> Relayed
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (read, write) = tokio::io::split(stream);
    let mut service = FrameReader::new(read);
    let (checked_in, checked_out) = tokio::io::duplex(PUMP_BUFFER);
    let mut target = (FrameReader::new(checked_out), FrameWriter::new(write));
    let checking = checked(&mut service, FrameWriter::new(checked_in));
    let (pipe_in, pipe_out) = tokio::io::duplex(PUMP_BUFFER);
    let mut client = FrameReader::new(pipe_out);
    let ending = Ending::default();
    let writing = ClientWrite::default();
    let pump = pump(
        reader,
        FrameWriter::new(pipe_in),
        ((stop, service_stop), &ending),
    );
    let relaying = relay(
        (&mut client, writer),
        (&mut target.0, &mut target.1),
        (before_outcome, &writing, FORWARD_STALL),
    );
    let cancelled = async {
        tokio::select! {
            () = ending.cancelling.cancelled() => {}
            () = stop.cancelled() => {}
            () = service_stop.cancelled() => {}
        }
    };
    let grace = tokio::time::sleep(Duration::MAX);
    tokio::pin!(pump, checking, relaying, cancelled, grace);
    let (mut pumping, mut cancelling) = ((true, true), false);
    loop {
        tokio::select! {
            relayed = &mut relaying => return relayed,
            () = &mut pump, if pumping.0 => pumping.0 = false,
            () = &mut checking, if pumping.1 => pumping.1 = false,
            () = &mut cancelled, if !cancelling => {
                cancelling = true;
                grace.as_mut().reset(tokio::time::Instant::now() + STUCK_GRACE);
            }
            () = &mut grace, if cancelling => {
                // Dropping the relay may cut a frame toward the caller short; then nothing
                // may follow it on that stream.
                return Relayed {
                    terminal: writing.terminal(),
                    to_target: 0,
                    from_target: 0,
                    client_present: !ending.left.load(Ordering::Relaxed) && !writing.torn(),
                };
            }
        }
    }
}

fn before_outcome() {
    fault("before_outcome_relayed");
}

/// Forwards the caller's frames to the relay, and turns a stop into the caller's `cancel`.
///
/// Returning closes the relay's client side, which the relay reads as the caller leaving.
async fn pump<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    mut pipe: FrameWriter<DuplexStream>,
    ((stop, service_stop), ending): ((&CancellationToken, &CancellationToken), &Ending),
) {
    let mut input_ended = false;
    loop {
        let next = tokio::select! {
            biased;
            () = stop.cancelled() => None,
            () = service_stop.cancelled() => None,
            next = reader.next() => Some(next),
        };
        let Some(Ok(Some(frame))) = next else {
            ending.cancelling.cancel();
            if next.is_none() {
                let _cancelled = async {
                    pipe.frame(FrameKind::Cancel, &[]).await?;
                    pipe.flush().await
                }
                .await;
            } else {
                ending.left.store(true, Ordering::Relaxed);
            }
            return;
        };
        // Caller data or an `end` on another channel than input, or after its `end` of input,
        // cancels the call, as any other frame the caller may not send does.
        let caller_input = match frame.kind() {
            FrameKind::Data => {
                !input_ended
                    && frame
                        .data()
                        .is_ok_and(|(channel, _)| channel == Channel::Input)
            }
            FrameKind::End => {
                frame.end() == Ok(Channel::Input) && !std::mem::replace(&mut input_ended, true)
            }
            _ => true,
        };
        let (kind, payload) = if caller_input {
            (frame.kind(), frame.payload())
        } else {
            (FrameKind::Cancel, &[][..])
        };
        if kind == FrameKind::Cancel {
            ending.cancelling.cancel();
        }
        let forwarded = async {
            pipe.frame(kind, payload).await?;
            pipe.flush().await
        };
        let stalled = tokio::time::sleep(INPUT_STALL);
        tokio::pin!(forwarded, stalled);
        let forwarded = tokio::select! {
            biased;
            forwarded = &mut forwarded => forwarded,
            () = &mut stalled => {
                ending.cancelling.cancel();
                forwarded.await
            }
        };
        if forwarded.is_err() {
            return;
        }
    }
}

/// Forwards the service's frames to the relay while they follow Services v1: one `accept`
/// before any `data`, `end`, or `control`, and `data` and `end` only on the output and error
/// channels, each until its `end`. Ending the stream at the first other frame makes the relay
/// report the call lost.
async fn checked<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    mut pipe: FrameWriter<DuplexStream>,
) {
    let (mut accepted, mut ended) = (false, (false, false));
    // Whether the service may still send on `channel`; an `end` when `ends`, which closes it.
    let mut open = |channel, ends| {
        let ended = match channel {
            Channel::Output => &mut ended.0,
            Channel::Error => &mut ended.1,
            Channel::Input => return false,
        };
        !std::mem::replace(ended, *ended || ends)
    };
    while let Ok(Some(frame)) = reader.next().await {
        let valid = match frame.kind() {
            FrameKind::Accept => !std::mem::replace(&mut accepted, true),
            FrameKind::Data => {
                accepted && frame.data().is_ok_and(|(channel, _)| open(channel, false))
            }
            FrameKind::End => accepted && frame.end().is_ok_and(|channel| open(channel, true)),
            FrameKind::Control => accepted,
            // The relay itself checks every other frame.
            _ => true,
        };
        if !valid || forward(&mut pipe, &frame).await.is_err() {
            return;
        }
    }
}

async fn forward(
    pipe: &mut FrameWriter<DuplexStream>,
    frame: &Frame,
) -> Result<(), ma2a_core::service::ServiceError> {
    pipe.frame(frame.kind(), frame.payload()).await?;
    pipe.flush().await
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "relay_rules_tests.rs"]
mod rules_tests;
