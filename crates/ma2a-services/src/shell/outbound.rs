//! Outbound terminal frame streaming, exit draining, and stop handling for one Shell session.

use std::time::Duration;

use ma2a_core::service::{Channel, EndReason, ServiceError, Termination};
use tokio::{
    io::AsyncWrite,
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::pty::Terminator;
use crate::{FrameWriter, reply, report::len64};

/// How long output is still drained after the program exits.
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Output side of one session and its stop conditions.
pub(super) struct Outbound<'a> {
    pub(super) output: &'a mut mpsc::Receiver<Vec<u8>>,
    pub(super) exited: &'a mut oneshot::Receiver<Termination>,
    pub(super) deadline: Instant,
    pub(super) shutdown: &'a CancellationToken,
    pub(super) cancelled: oneshot::Receiver<ServiceError>,
    pub(super) done: oneshot::Sender<()>,
    pub(super) terminator: &'a mut Option<Terminator>,
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

struct SessionState<'a> {
    ended: Ended,
    exit: Option<Termination>,
    drain_until: Option<Instant>,
    done: Option<oneshot::Sender<()>>,
    terminator: &'a mut Option<Terminator>,
}

impl SessionState<'_> {
    async fn finish_and_terminate<F>(
        &mut self,
        write: std::pin::Pin<&mut F>,
        (chunk_len, leader_running): (usize, bool),
    ) where
        F: Future<Output = Result<(), ServiceError>>,
    {
        if let Some(done) = self.done.take() {
            let _finished = done.send(());
        }
        let terminate = async {
            if let Some(terminator) = self.terminator.take() {
                terminator.terminate(leader_running).await;
            }
        };
        let ((), finished) = tokio::join!(terminate, reply::finish_frame(write));
        if finished {
            self.ended.output_bytes += len64(chunk_len);
        }
    }

    async fn write_chunk<W, S>(
        &mut self,
        (writer, exited, stop): (
            &mut FrameWriter<W>,
            &mut oneshot::Receiver<Termination>,
            &mut std::pin::Pin<&mut S>,
        ),
        chunk: &[u8],
    ) -> bool
    where
        W: AsyncWrite + Unpin,
        S: Future<Output = (EndReason, Option<ServiceError>)>,
    {
        let write = writer.data(Channel::Output, chunk);
        tokio::pin!(write);
        // A caller that stops reading cannot hold the session open past the drain after exit.
        let stop_leader = loop {
            tokio::select! {
                biased;
                (reason, error) = &mut *stop => {
                    (self.ended.reason, self.ended.stream_error) = (reason, error);
                    if error.is_some() {
                        return false;
                    }
                    break reason != EndReason::Completed
                        || self.exit.is_none_or(|t| t == Termination::Unknown);
                }
                termination = &mut *exited, if self.exit.is_none() => {
                    self.exit = Some(termination.unwrap_or(Termination::Unknown));
                    self.drain_until = Some(Instant::now() + EXIT_DRAIN_GRACE);
                }
                () = tokio::time::sleep_until(self.drain_until.unwrap_or_else(Instant::now)), if self.drain_until.is_some() => {
                    break self.exit.is_none_or(|t| t == Termination::Unknown);
                }
                written = &mut write => {
                    if let Err(error) = written {
                        (self.ended.reason, self.ended.stream_error) =
                            (EndReason::Cancelled, Some(error));
                        return false;
                    }
                    self.ended.output_bytes += len64(chunk.len());
                    return true;
                }
            }
        };
        self.finish_and_terminate(write, (chunk.len(), stop_leader))
            .await;
        false
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
        terminator,
    } = outbound;
    let stop = stop_reason(deadline, shutdown, cancelled);
    tokio::pin!(stop);
    let mut state = SessionState {
        ended: Ended {
            reason: EndReason::Completed,
            termination: Termination::Unknown,
            output_bytes: 0,
            stream_error: None,
        },
        exit: None,
        drain_until: None,
        done: Some(done),
        terminator,
    };
    // A closed output answers every receive at once; it is not polled again.
    let mut output_closed = false;
    loop {
        let chunk = tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                (state.ended.reason, state.ended.stream_error) = (reason, error);
                break;
            }
            termination = &mut *exited, if state.exit.is_none() => {
                state.exit = Some(termination.unwrap_or(Termination::Unknown));
                if output_closed {
                    break;
                }
                state.drain_until = Some(Instant::now() + EXIT_DRAIN_GRACE);
                continue;
            }
            () = tokio::time::sleep_until(state.drain_until.unwrap_or_else(Instant::now)), if state.drain_until.is_some() => break,
            chunk = output.recv(), if !output_closed => chunk,
        };
        let Some(chunk) = chunk else {
            if state.exit.is_some() {
                break;
            }
            // A program may close its terminal and keep running; the session ends when it exits.
            output_closed = true;
            continue;
        };
        tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                (state.ended.reason, state.ended.stream_error) = (reason, error);
                break;
            }
            () = std::future::ready(()) => {}
        }
        if !state.write_chunk((writer, exited, &mut stop), &chunk).await {
            break;
        }
    }
    state.ended.termination = state.exit.unwrap_or(Termination::Unknown);
    if let Some(done) = state.done.take() {
        let _finished = done.send(());
    }
    state.ended
}
