//! Relaying one delivered call between its caller and its service until a terminal.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use ma2a_core::service::FrameKind;
use ma2a_services::{FrameReader, FrameWriter};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream};
use tokio_util::sync::CancellationToken;

use super::super::{CallIo, fault};
use crate::stream_plane::relay::{CANCEL_GRACE, Relayed, Terminal, relay};

/// Bytes buffered between the caller's frames and the relay.
const PUMP_BUFFER: usize = 128 * 1024;
/// How long a call may take to end once it is being cancelled, even if its service stopped
/// reading: one second past the relay's own grace, so the relay ends every other call first.
const STUCK_GRACE: Duration = CANCEL_GRACE.saturating_add(Duration::from_secs(1));

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
    let mut target = (FrameReader::new(read), FrameWriter::new(write));
    let (pipe_in, pipe_out) = tokio::io::duplex(PUMP_BUFFER);
    let mut client = FrameReader::new(pipe_out);
    let ending = Ending::default();
    let pump = pump(
        reader,
        FrameWriter::new(pipe_in),
        ((stop, service_stop), &ending),
    );
    let relaying = relay(
        (&mut client, writer),
        (&mut target.0, &mut target.1),
        before_outcome,
    );
    let cancelled = async {
        tokio::select! {
            () = ending.cancelling.cancelled() => {}
            () = stop.cancelled() => {}
            () = service_stop.cancelled() => {}
        }
    };
    let grace = tokio::time::sleep(Duration::MAX);
    tokio::pin!(pump, relaying, cancelled, grace);
    let (mut pumping, mut cancelling) = (true, false);
    loop {
        tokio::select! {
            biased;
            relayed = &mut relaying => return relayed,
            () = &mut pump, if pumping => pumping = false,
            () = &mut cancelled, if !cancelling => {
                cancelling = true;
                grace.as_mut().reset(tokio::time::Instant::now() + STUCK_GRACE);
            }
            () = &mut grace, if cancelling => {
                return Relayed {
                    terminal: Terminal::Lost,
                    to_target: 0,
                    from_target: 0,
                    client_present: !ending.left.load(Ordering::Relaxed),
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
        if frame.kind() == FrameKind::Cancel {
            ending.cancelling.cancel();
        }
        let forwarded = async {
            pipe.frame(frame.kind(), frame.payload()).await?;
            pipe.flush().await
        };
        if forwarded.await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
