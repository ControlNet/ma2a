//! Verbatim frame relay between a local stream-plane client and one remote target.
//!
//! Every buffer is one bounded frame, so flow control on either side reaches the other.
//! The relay decides nothing about replay; it only reports how the target ended.

use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use ma2a_core::service::{FRAME_HEADER_LEN, FrameHeader, FrameKind, ServiceError, outcome_status};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Notify,
};

/// How long a cancelled target may take to report its terminal before it counts as lost.
pub(crate) const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How long the last frame and the shutdown toward a client may take; a client that stopped
/// reading must not hold an ended operation open.
pub(crate) const FINAL_WRITE_GRACE: Duration = Duration::from_secs(5);
/// How long one frame toward the target or client may wait for room by default, as long as a
/// host waits for progress; a side that takes nothing for longer is stalled and gets cancelled.
pub(crate) const FORWARD_STALL: Duration = Duration::from_secs(30);

/// What an owner that drops a relay can still learn from it: whether a frame toward the client
/// is being written right now, and the target's terminal once it was read.
///
/// A relay dropped while a write is under way may have left the client's stream inside a
/// frame, so nothing may be appended to that stream afterwards. A terminal read before the
/// drop stays known even when forwarding it to the client never finished.
#[derive(Debug, Default)]
pub(crate) struct ClientWrite {
    writing: AtomicBool,
    terminal: OnceLock<Terminal>,
}

impl ClientWrite {
    /// Returns whether the client's stream may end inside a frame.
    pub(crate) fn torn(&self) -> bool {
        self.writing.load(Ordering::Relaxed)
    }

    /// Returns the target's terminal if it was read, or [`Terminal::Lost`].
    pub(crate) fn terminal(&self) -> Terminal {
        self.terminal.get().cloned().unwrap_or(Terminal::Lost)
    }
}

/// Runs the last write or shutdown toward a client within [`FINAL_WRITE_GRACE`].
pub(crate) async fn bounded<T>(write: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(FINAL_WRITE_GRACE, write).await.ok()
}

/// How the target ended the operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    /// The target refused before any effect.
    Rejected(ServiceError),
    /// The target reported its terminal outcome; `frame` is the exact encoded frame.
    Outcome {
        frame: Vec<u8>,
        failure: Option<ServiceError>,
    },
    /// The stream ended without a terminal, so the effect is unknown.
    Lost,
}

/// Terminal and payload bytes moved in each direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Relayed {
    pub(crate) terminal: Terminal,
    pub(crate) to_target: u64,
    pub(crate) from_target: u64,
    /// Whether the client is still there to receive a frame the caller sends.
    pub(crate) client_present: bool,
}

/// Encodes one frame exactly as it travels.
pub(crate) fn encoded(kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>, ServiceError> {
    let header = FrameHeader::new(kind, payload.len())
        .and_then(FrameHeader::encode)
        .map_err(|_| ServiceError::Internal)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Frames a client may send after `open`.
pub(super) const fn from_client(kind: FrameKind) -> bool {
    matches!(
        kind,
        FrameKind::Data | FrameKind::End | FrameKind::Control | FrameKind::Cancel
    )
}

/// Frames a target may send before its terminal.
const fn from_target(kind: FrameKind) -> bool {
    matches!(
        kind,
        FrameKind::Accept | FrameKind::Data | FrameKind::End | FrameKind::Control
    )
}

fn data_len(frame: &Frame) -> u64 {
    frame.data().map_or(0, |(_, bytes)| {
        u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    })
}

/// Writes one frame within `bound`.
pub(super) async fn forward<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    frame: &Frame,
    bound: Duration,
) -> Result<(), ServiceError> {
    let sent = async {
        writer.frame(frame.kind(), frame.payload()).await?;
        writer.flush().await
    };
    tokio::time::timeout(bound, sent)
        .await
        .unwrap_or(Err(ServiceError::TimedOut))
}

/// State both directions of one relay update.
#[derive(Debug, Default)]
pub(super) struct Shared {
    pub(super) to_target: AtomicU64,
    from_target: AtomicU64,
    pub(super) client_gone: AtomicBool,
    /// Raised by the target direction when the client can no longer receive frames.
    pub(super) client_lost: Notify,
}

impl Shared {
    pub(super) fn count(counter: &AtomicU64, frame: &Frame) {
        counter.fetch_add(data_len(frame), Ordering::Relaxed);
    }
}

#[path = "relay_upstream.rs"]
mod upstream;

/// Relays frames after the client's `open` was written to the target, until the target's
/// terminal, its loss, or the cancel grace after the client cancelled or disappeared.
///
/// The two directions run independently, so neither side blocking on a full stream can
/// stall the other, and a full-duplex operation can never deadlock inside the relay.
/// `before_outcome` runs once the target's outcome is read and before it is forwarded,
/// `writing` tells an owner that drops this future whether a client frame was cut short, and
/// `stall` bounds how long one non-terminal frame may wait for room in either direction.
pub(crate) async fn relay<CR, CW, TR, TW>(
    client: (&mut FrameReader<CR>, &mut FrameWriter<CW>),
    target: (&mut FrameReader<TR>, &mut FrameWriter<TW>),
    (before_outcome, writing, stall): (fn(), &ClientWrite, Duration),
) -> Relayed
where
    CR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin,
    TR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
{
    relay_watching(
        client,
        target,
        (before_outcome, writing, stall, std::future::pending()),
    )
    .await
}

/// Like [`relay`], and also raises `client_lost` if `disconnected` resolves out of band (for
/// example when an IPC socket closes while one lookahead frame is held).
pub(crate) async fn relay_watching<CR, CW, TR, TW, D>(
    (client_reader, client_writer): (&mut FrameReader<CR>, &mut FrameWriter<CW>),
    (target_reader, target_writer): (&mut FrameReader<TR>, &mut FrameWriter<TW>),
    (before_outcome, writing, stall, disconnected): (fn(), &ClientWrite, Duration, D),
) -> Relayed
where
    CR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin,
    TR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
    D: Future<Output = ()>,
{
    let shared = Shared::default();
    let downstream = downstream(
        (target_reader, client_writer),
        (&shared, before_outcome, writing, stall),
    );
    let upstream = upstream::run((client_reader, target_writer), (&shared, stall));
    let grace = tokio::time::sleep(Duration::MAX);
    tokio::pin!(downstream, upstream, grace, disconnected);
    let mut cancelling = false;
    let mut peer_gone = false;
    let terminal = loop {
        tokio::select! {
            terminal = &mut downstream => break terminal,
            () = &mut disconnected, if !peer_gone => {
                peer_gone = true;
                shared.client_gone.store(true, Ordering::Relaxed);
                shared.client_lost.notify_one();
            }
            wait = &mut upstream, if !cancelling => {
                // A target that took no write within its bound has used up the grace already.
                if !wait {
                    break writing.terminal();
                }
                cancelling = true;
                grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
            }
            () = &mut grace, if cancelling => break writing.terminal(),
        }
    };
    Relayed {
        terminal,
        to_target: shared.to_target.load(Ordering::Relaxed),
        from_target: shared.from_target.load(Ordering::Relaxed),
        client_present: !shared.client_gone.load(Ordering::Relaxed) && !writing.torn(),
    }
}

/// Target → client until the target's terminal; a malformed or missing one is a loss.
async fn downstream<TR, CW>(
    (target_reader, client_writer): (&mut FrameReader<TR>, &mut FrameWriter<CW>),
    (shared, before_outcome, writing, stall): (&Shared, fn(), &ClientWrite, Duration),
) -> Terminal
where
    TR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin,
{
    // Whether any frame other than a terminal was forwarded already.
    let mut started = false;
    loop {
        let Ok(Some(frame)) = target_reader.next().await else {
            return Terminal::Lost;
        };
        let terminal = match frame.kind() {
            // A rejection promises that nothing happened, which no longer holds once the
            // target accepted or sent anything else; such a stream is indeterminate and is
            // never replayed as refused.
            FrameKind::Reject if started => return Terminal::Lost,
            FrameKind::Reject => match frame.rejection() {
                Ok(error) => Some(Terminal::Rejected(error)),
                Err(_) => return Terminal::Lost,
            },
            FrameKind::Outcome => match (
                encoded(FrameKind::Outcome, frame.payload()),
                outcome_status(frame.payload()),
            ) {
                (Ok(bytes), Ok(failure)) => Some(Terminal::Outcome {
                    frame: bytes,
                    failure,
                }),
                _ => return Terminal::Lost,
            },
            kind if from_target(kind) => {
                started = true;
                Shared::count(&shared.from_target, &frame);
                None
            }
            _ => return Terminal::Lost,
        };
        if let Some(terminal) = &terminal {
            let _kept = writing.terminal.set(terminal.clone());
        }
        if matches!(terminal, Some(Terminal::Outcome { .. })) {
            before_outcome();
        }
        if !shared.client_gone.load(Ordering::Relaxed) {
            writing.writing.store(true, Ordering::Relaxed);
            // The frames that open and end an operation have the final-write bound: a client
            // that does not take them must not hold the operation for the stall bound.
            let bound = if terminal.is_some() || frame.kind() == FrameKind::Accept {
                FINAL_WRITE_GRACE
            } else {
                stall
            };
            let forwarded = forward(client_writer, &frame, bound).await;
            writing.writing.store(false, Ordering::Relaxed);
            if forwarded.is_err() {
                shared.client_gone.store(true, Ordering::Relaxed);
                shared.client_lost.notify_one();
            }
        }
        if let Some(terminal) = terminal {
            return terminal;
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "relay_grace_tests.rs"]
mod grace_tests;

#[cfg(test)]
#[path = "relay_disconnect_tests.rs"]
mod disconnect_tests;
