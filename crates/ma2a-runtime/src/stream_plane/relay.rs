//! Verbatim frame relay between a local stream-plane client and one remote target.
//!
//! Every buffer is one bounded frame, so flow control on either side reaches the other.
//! The relay decides nothing about replay; it only reports how the target ended.

use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
const fn from_client(kind: FrameKind) -> bool {
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

async fn forward<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    frame: &Frame,
) -> Result<(), ServiceError> {
    writer.frame(frame.kind(), frame.payload()).await?;
    writer.flush().await
}

/// State both directions of one relay update.
#[derive(Debug, Default)]
struct Shared {
    to_target: AtomicU64,
    from_target: AtomicU64,
    client_gone: AtomicBool,
    /// Raised by the target direction when the client can no longer receive frames.
    client_lost: Notify,
}

impl Shared {
    fn count(counter: &AtomicU64, frame: &Frame) {
        counter.fetch_add(data_len(frame), Ordering::Relaxed);
    }
}

/// Relays frames after the client's `open` was written to the target, until the target's
/// terminal, its loss, or the cancel grace after the client cancelled or disappeared.
///
/// The two directions run independently, so neither side blocking on a full stream can
/// stall the other, and a full-duplex operation can never deadlock inside the relay.
/// `before_terminal` runs once the target's terminal is read and before it is forwarded.
pub(crate) async fn relay<CR, CW, TR, TW>(
    (client_reader, client_writer): (&mut FrameReader<CR>, &mut FrameWriter<CW>),
    (target_reader, target_writer): (&mut FrameReader<TR>, &mut FrameWriter<TW>),
    before_terminal: fn(),
) -> Relayed
where
    CR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin,
    TR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
{
    let shared = Shared::default();
    let downstream = downstream((target_reader, client_writer), (&shared, before_terminal));
    let upstream = upstream((client_reader, target_writer), &shared);
    let grace = tokio::time::sleep(Duration::MAX);
    tokio::pin!(downstream, upstream, grace);
    let mut cancelling = false;
    let terminal = loop {
        tokio::select! {
            biased;
            terminal = &mut downstream => break terminal,
            () = &mut upstream, if !cancelling => {
                cancelling = true;
                grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
            }
            () = &mut grace, if cancelling => break Terminal::Lost,
        }
    };
    Relayed {
        terminal,
        to_target: shared.to_target.load(Ordering::Relaxed),
        from_target: shared.from_target.load(Ordering::Relaxed),
        client_present: !shared.client_gone.load(Ordering::Relaxed),
    }
}

/// Client → target until the client cancels, breaks the contract, or disappears; each of
/// those leaves the target cancelled. Returning starts the cancel grace.
async fn upstream<CR, TW>(
    (client_reader, target_writer): (&mut FrameReader<CR>, &mut FrameWriter<TW>),
    shared: &Shared,
) where
    CR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
{
    loop {
        let frame = tokio::select! {
            biased;
            () = shared.client_lost.notified() => None,
            frame = client_reader.next() => Some(frame),
        };
        match frame {
            Some(Ok(Some(frame))) if from_client(frame.kind()) => {
                Shared::count(&shared.to_target, &frame);
                if forward(target_writer, &frame).await.is_err()
                    || frame.kind() == FrameKind::Cancel
                {
                    return;
                }
            }
            other => {
                // A protocol violation leaves the client able to hear the terminal; a closed
                // connection, a read failure, or a lost client does not.
                if !matches!(other, Some(Ok(Some(_)))) {
                    shared.client_gone.store(true, Ordering::Relaxed);
                }
                let _cancelled = async {
                    target_writer.frame(FrameKind::Cancel, &[]).await?;
                    target_writer.flush().await
                }
                .await;
                return;
            }
        }
    }
}

/// Target → client until the target's terminal; a malformed or missing one is a loss.
async fn downstream<TR, CW>(
    (target_reader, client_writer): (&mut FrameReader<TR>, &mut FrameWriter<CW>),
    (shared, before_terminal): (&Shared, fn()),
) -> Terminal
where
    TR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin,
{
    let mut accepted = false;
    loop {
        let Ok(Some(frame)) = target_reader.next().await else {
            return Terminal::Lost;
        };
        let terminal = match frame.kind() {
            // A rejection promises that nothing happened, which no longer holds once the
            // target accepted; such a stream is indeterminate and is never replayed as refused.
            FrameKind::Reject if accepted => return Terminal::Lost,
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
                accepted |= kind == FrameKind::Accept;
                Shared::count(&shared.from_target, &frame);
                None
            }
            _ => return Terminal::Lost,
        };
        if terminal.is_some() {
            before_terminal();
        }
        if !shared.client_gone.load(Ordering::Relaxed)
            && forward(client_writer, &frame).await.is_err()
        {
            shared.client_gone.store(true, Ordering::Relaxed);
            shared.client_lost.notify_one();
        }
        if let Some(terminal) = terminal {
            return terminal;
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
