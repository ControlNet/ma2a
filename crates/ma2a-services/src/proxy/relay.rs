//! Bidirectional relay with half-close, idle, lifetime, cancellation, and shutdown.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use ma2a_core::service::{
    Channel, EndReason, FrameKind, MAX_DATA_CHUNK_LEN, MAX_OPERATION_LIFETIME_MS, ServiceError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::TcpStream,
    sync::Notify,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{FrameReader, FrameWriter, input::Input, reply, report::len64};

/// How one relay ended.
#[derive(Debug)]
pub(super) struct Relayed {
    pub(super) result: Result<EndReason, ServiceError>,
    pub(super) to_target: u64,
    pub(super) from_target: u64,
    pub(super) stream_error: Option<ServiceError>,
}

/// A direction stopped for a reason that ends the whole relay.
#[derive(Clone, Copy)]
enum Stop {
    /// The caller cancelled or its stream was lost.
    Caller(ServiceError),
    /// The destination socket failed.
    Target,
    /// A write to the destination made no progress for the idle timeout.
    Idle,
}

struct Counters {
    to_target: AtomicU64,
    from_target: AtomicU64,
    last_activity: AtomicU64,
    started: Instant,
    idle: Duration,
    /// Signalled when the destination side has delivered everything to the caller.
    delivered: Notify,
}

impl Counters {
    fn touch(&self) {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_activity.store(elapsed, Ordering::Relaxed);
    }
}

pub(super) async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    stream: TcpStream,
    caller: (&mut FrameReader<R>, &mut FrameWriter<W>),
    limits: (Duration, &CancellationToken),
) -> Relayed {
    relay_streams(stream.into_split(), caller, limits).await
}

pub(super) async fn relay_streams<
    TR: AsyncRead + Unpin,
    TW: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
>(
    (target_read, target_write): (TR, TW),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (idle, shutdown): (Duration, &CancellationToken),
) -> Relayed {
    let counters = Counters {
        to_target: AtomicU64::new(0),
        from_target: AtomicU64::new(0),
        last_activity: AtomicU64::new(0),
        started: Instant::now(),
        idle,
        delivered: Notify::new(),
    };
    let stop = CancellationToken::new();
    let lifetime = tokio::time::sleep_until(
        counters.started + Duration::from_millis(MAX_OPERATION_LIFETIME_MS),
    );
    let idle = idle_expired(&counters);
    let upstream = upstream(reader, target_write, &counters);
    let downstream = downstream(target_read, writer, (&counters, &stop));
    tokio::pin!(lifetime, idle, upstream, downstream);
    let (mut up_done, mut down_done) = (false, false);
    let (result, stream_error) = loop {
        if shutdown.is_cancelled() {
            break (Ok(EndReason::Shutdown), None);
        }
        if up_done && down_done {
            break (Ok(EndReason::Completed), None);
        }
        tokio::select! {
            () = shutdown.cancelled() => break (Ok(EndReason::Shutdown), None),
            () = &mut lifetime => break (Ok(EndReason::TimedOut), None),
            () = &mut idle => break (Ok(EndReason::TimedOut), None),
            res = &mut upstream, if !up_done => {
                up_done = true;
                if let Err(stopped) = res {
                    break stop_outcome(stopped);
                }
            }
            res = &mut downstream, if !down_done => {
                down_done = true;
                if let Err(stopped) = res {
                    break stop_outcome(stopped);
                }
            }
        }
    };
    if !down_done && stream_error.is_none() {
        stop.cancel();
        let _finished = downstream.await;
    }
    Relayed {
        result,
        to_target: counters.to_target.load(Ordering::Relaxed),
        from_target: counters.from_target.load(Ordering::Relaxed),
        stream_error,
    }
}

const fn stop_outcome(stopped: Stop) -> (Result<EndReason, ServiceError>, Option<ServiceError>) {
    match stopped {
        Stop::Caller(ServiceError::Cancelled) => (Ok(EndReason::Cancelled), None),
        Stop::Caller(error) => (Ok(EndReason::Cancelled), Some(error)),
        Stop::Target => (Err(ServiceError::Unreachable), None),
        Stop::Idle => (Ok(EndReason::TimedOut), None),
    }
}

async fn idle_expired(counters: &Counters) {
    let idle_ms = u64::try_from(counters.idle.as_millis()).unwrap_or(u64::MAX);
    loop {
        let last = counters.last_activity.load(Ordering::Relaxed);
        let due = counters.started + Duration::from_millis(last.saturating_add(idle_ms));
        tokio::time::sleep_until(due).await;
        if counters.last_activity.load(Ordering::Relaxed) == last {
            return;
        }
    }
}

/// Caller → destination; `end` shuts down the TCP write side, and an early stream close cancels.
async fn upstream<R: AsyncRead + Unpin, TW: AsyncWrite + Unpin>(
    reader: &mut FrameReader<R>,
    mut target: TW,
    counters: &Counters,
) -> Result<(), Stop> {
    let mut input = Input::new(reader);
    loop {
        let frame = match input.next().await {
            Ok(Some(frame)) => frame,
            // A caller keeps its side open until the outcome; closing it earlier cancels.
            Ok(None) => return Err(Stop::Caller(ServiceError::Cancelled)),
            Err(error) => return Err(Stop::Caller(error)),
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    return Err(Stop::Caller(ServiceError::InvalidInput));
                };
                write((&mut input, &mut target), bytes, counters).await?;
            }
            FrameKind::End if frame.end() == Ok(Channel::Input) => break,
            FrameKind::Cancel => return Err(Stop::Caller(ServiceError::Cancelled)),
            _ => return Err(Stop::Caller(ServiceError::InvalidInput)),
        }
    }
    target.shutdown().await.map_err(|_| Stop::Target)?;
    // The caller still holds its stream open: a later cancel, close, or reset must end the relay
    // at once rather than when a silent destination finally reaches the idle limit.
    let frame = tokio::select! {
        biased;
        () = counters.delivered.notified() => return Ok(()),
        frame = input.next() => frame,
    };
    match frame {
        Ok(Some(frame)) if frame.kind() != FrameKind::Cancel => {
            Err(Stop::Caller(ServiceError::InvalidInput))
        }
        Ok(_) => Err(Stop::Caller(ServiceError::Cancelled)),
        Err(error) => Err(Stop::Caller(error)),
    }
}

/// Writes `bytes` to the destination while still reading ahead for a caller `cancel` or close.
/// A write that stays blocked for the idle timeout ends the relay even while the destination keeps
/// sending, so one that never reads cannot hold it.
async fn write<R: AsyncRead + Unpin, TW: AsyncWrite + Unpin>(
    (input, target): (&mut Input<'_, R>, &mut TW),
    bytes: &[u8],
    counters: &Counters,
) -> Result<(), Stop> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = input
            .write_within(target.write(rest), counters.idle)
            .await
            .ok_or(Stop::Idle)?
            .map_err(Stop::Caller)?
            .map_err(|_| Stop::Target)?;
        if written == 0 {
            return Err(Stop::Target);
        }
        rest = rest.get(written..).unwrap_or_default();
        counters
            .to_target
            .fetch_add(len64(written), Ordering::Relaxed);
        counters.touch();
    }
    Ok(())
}

enum Sent {
    Done,
    Stopped(bool),
}

async fn write_or_stop<F: Future<Output = Result<(), ServiceError>>>(
    write: F,
    stop: &CancellationToken,
) -> Result<Sent, Stop> {
    tokio::pin!(write);
    tokio::select! {
        biased;
        () = stop.cancelled() => Ok(Sent::Stopped(reply::finish_frame(&mut write).await)),
        written = &mut write => written.map(|()| Sent::Done).map_err(Stop::Caller),
    }
}

/// Destination → caller; destination EOF becomes `end`.
async fn downstream<TR: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut target: TR,
    writer: &mut FrameWriter<W>,
    (counters, stop): (&Counters, &CancellationToken),
) -> Result<(), Stop> {
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = tokio::select! {
            biased;
            () = stop.cancelled() => return Ok(()),
            read = target.read(&mut buffer) => read.map_err(|_| Stop::Target)?,
        };
        if stop.is_cancelled() {
            return Ok(());
        }
        if read == 0 {
            if matches!(
                write_or_stop(writer.end(Channel::Input), stop).await?,
                Sent::Done
            ) {
                counters.delivered.notify_one();
            }
            return Ok(());
        }
        let bytes = buffer.get(..read).unwrap_or_default();
        let sent = write_or_stop(writer.data(Channel::Input, bytes), stop).await?;
        if matches!(sent, Sent::Done | Sent::Stopped(true)) {
            counters
                .from_target
                .fetch_add(len64(read), Ordering::Relaxed);
        }
        if matches!(sent, Sent::Stopped(_)) {
            return Ok(());
        }
        counters.touch();
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
