//! Frame relay between one caller stream and one terminal session.

use std::{sync::Mutex, time::Duration};

use ma2a_core::service::{Channel, EndReason, FrameKind, ServiceError, TerminalSize, Termination};
use portable_pty::MasterPty;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::pty::pty_size;
use crate::{
    FrameReader, FrameWriter,
    input::{Input, Wrote},
    reply,
    report::len64,
};

/// How long output is still drained after the program exits.
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Forwards input and resize requests until the session ends, returning input bytes.
pub(super) async fn inbound<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    (input, master): (mpsc::Sender<Vec<u8>>, &Mutex<Box<dyn MasterPty + Send>>),
    (cancel, mut finished): (oneshot::Sender<ServiceError>, oneshot::Receiver<()>),
) -> u64 {
    let mut reader = Input::new(reader);
    let mut input = Some(input);
    let mut input_bytes = 0_u64;
    // Set once the terminal stops taking input; the caller may not know yet and keeps sending.
    let mut closed_by_session = false;
    let failure = loop {
        let frame = tokio::select! {
            _ = &mut finished => return input_bytes,
            frame = reader.next() => frame,
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            // A caller keeps its side open until the outcome; closing it earlier cancels.
            Ok(None) => break ServiceError::Cancelled,
            Err(error) => break error,
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    break ServiceError::InvalidInput;
                };
                let Some(sender) = input.as_ref() else {
                    if closed_by_session {
                        continue;
                    }
                    break ServiceError::InvalidInput;
                };
                input_bytes += len64(bytes.len());
                let chunk = bytes.to_vec();
                let sent = match reader.write(sender.send(chunk), &mut finished).await {
                    Wrote::Done(sent) => sent,
                    Wrote::Stopped(error) => break error,
                    Wrote::Finished => return input_bytes,
                };
                if sent.is_err() {
                    input = None;
                    closed_by_session = true;
                }
            }
            FrameKind::Control => match TerminalSize::decode_resize(frame.payload()) {
                Ok(size) => {
                    let _resized = master
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .resize(pty_size(size));
                }
                Err(_) => break ServiceError::InvalidInput,
            },
            FrameKind::End if frame.end() == Ok(Channel::Input) => {
                input = None;
                closed_by_session = false;
            }
            FrameKind::Cancel => break ServiceError::Cancelled,
            _ => break ServiceError::InvalidInput,
        }
    };
    let _signalled = cancel.send(failure);
    let _finished = finished.await;
    input_bytes
}

/// Output side of one session and its stop conditions.
pub(super) struct Outbound<'a> {
    pub(super) output: &'a mut mpsc::Receiver<Vec<u8>>,
    pub(super) exited: &'a mut oneshot::Receiver<Termination>,
    pub(super) deadline: Instant,
    pub(super) shutdown: &'a CancellationToken,
    pub(super) cancelled: oneshot::Receiver<ServiceError>,
    pub(super) done: oneshot::Sender<()>,
}

/// Terminal facts of one session.
#[derive(Debug)]
pub(super) struct Ended {
    pub(super) reason: EndReason,
    pub(super) termination: Termination,
    pub(super) output_bytes: u64,
    pub(super) stream_error: Option<ServiceError>,
}

async fn stop_reason(
    deadline: Instant,
    shutdown: &CancellationToken,
    cancelled: oneshot::Receiver<ServiceError>,
) -> (EndReason, Option<ServiceError>) {
    let cancelled = async {
        match cancelled.await {
            Ok(ServiceError::Cancelled) => None,
            Ok(error) => Some(error),
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        () = shutdown.cancelled() => (EndReason::Shutdown, None),
        () = tokio::time::sleep_until(deadline) => (EndReason::TimedOut, None),
        error = cancelled => (EndReason::Cancelled, error),
    }
}

/// Streams terminal output until exit plus a short drain, or until a stop condition.
pub(super) async fn outbound<W: AsyncWrite + Unpin>(
    outbound: Outbound<'_>,
    writer: &mut FrameWriter<W>,
) -> Ended {
    let Outbound {
        output,
        exited,
        deadline,
        shutdown,
        cancelled,
        done,
    } = outbound;
    let stop = stop_reason(deadline, shutdown, cancelled);
    tokio::pin!(stop);
    let mut ended = Ended {
        reason: EndReason::Completed,
        termination: Termination::Unknown,
        output_bytes: 0,
        stream_error: None,
    };
    let mut exit: Option<Termination> = None;
    let mut drain_until: Option<Instant> = None;
    // A closed output answers every receive at once; it is not polled again.
    let mut output_closed = false;
    'session: loop {
        let chunk = tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                (ended.reason, ended.stream_error) = (reason, error);
                break;
            }
            termination = &mut *exited, if exit.is_none() => {
                exit = Some(termination.unwrap_or(Termination::Unknown));
                if output_closed {
                    break;
                }
                drain_until = Some(Instant::now() + EXIT_DRAIN_GRACE);
                continue;
            }
            () = tokio::time::sleep_until(drain_until.unwrap_or_else(Instant::now)), if drain_until.is_some() => break,
            chunk = output.recv(), if !output_closed => chunk,
        };
        let Some(chunk) = chunk else {
            if exit.is_some() {
                break;
            }
            // A program may close its terminal and keep running; the session ends when it exits.
            output_closed = true;
            continue;
        };
        tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                (ended.reason, ended.stream_error) = (reason, error);
                break;
            }
            () = std::future::ready(()) => {}
        }
        let write = writer.data(Channel::Output, &chunk);
        tokio::pin!(write);
        // A caller that stops reading cannot hold the session open past the drain after exit.
        let written = loop {
            tokio::select! {
                biased;
                (reason, error) = &mut stop => {
                    (ended.reason, ended.stream_error) = (reason, error);
                    if error.is_none() && reply::finish_frame(&mut write).await {
                        ended.output_bytes += len64(chunk.len());
                    }
                    break 'session;
                }
                termination = &mut *exited, if exit.is_none() => {
                    exit = Some(termination.unwrap_or(Termination::Unknown));
                    drain_until = Some(Instant::now() + EXIT_DRAIN_GRACE);
                }
                () = tokio::time::sleep_until(drain_until.unwrap_or_else(Instant::now)), if drain_until.is_some() => {
                    if reply::finish_frame(&mut write).await {
                        ended.output_bytes += len64(chunk.len());
                    }
                    break 'session;
                }
                written = &mut write => break written,
            }
        };
        if let Err(error) = written {
            (ended.reason, ended.stream_error) = (EndReason::Cancelled, Some(error));
            break;
        }
        ended.output_bytes += len64(chunk.len());
    }
    ended.termination = exit.unwrap_or(Termination::Unknown);
    let _finished = done.send(());
    ended
}

#[cfg(all(test, unix))]
#[path = "io_tests.rs"]
mod tests;
