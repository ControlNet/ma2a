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
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Notify,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{FrameReader, FrameWriter, report::len64};

/// How one relay ended.
#[derive(Debug)]
pub(super) struct Relayed {
    pub(super) result: Result<EndReason, ServiceError>,
    pub(super) to_target: u64,
    pub(super) from_target: u64,
    pub(super) stream_error: Option<ServiceError>,
}

/// A direction stopped for a reason that ends the whole relay.
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

pub(super) async fn run<R, W>(
    stream: TcpStream,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (idle, shutdown): (Duration, &CancellationToken),
) -> Relayed
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let counters = Counters {
        to_target: AtomicU64::new(0),
        from_target: AtomicU64::new(0),
        last_activity: AtomicU64::new(0),
        started: Instant::now(),
        idle,
        delivered: Notify::new(),
    };
    let (target_read, target_write) = stream.into_split();
    let lifetime = counters.started + Duration::from_millis(MAX_OPERATION_LIFETIME_MS);
    let both = async {
        tokio::try_join!(
            upstream(reader, target_write, &counters),
            downstream(target_read, writer, &counters),
        )
    };
    let (result, stream_error) = tokio::select! {
        biased;
        () = shutdown.cancelled() => (Ok(EndReason::Shutdown), None),
        () = tokio::time::sleep_until(lifetime) => (Ok(EndReason::TimedOut), None),
        () = idle_expired(&counters) => (Ok(EndReason::TimedOut), None),
        joined = both => match joined {
            Ok(((), ())) => (Ok(EndReason::Completed), None),
            Err(Stop::Caller(ServiceError::Cancelled)) => (Ok(EndReason::Cancelled), None),
            Err(Stop::Caller(error)) => (Ok(EndReason::Cancelled), Some(error)),
            Err(Stop::Target) => (Err(ServiceError::Unreachable), None),
            Err(Stop::Idle) => (Ok(EndReason::TimedOut), None),
        },
    };
    Relayed {
        result,
        to_target: counters.to_target.load(Ordering::Relaxed),
        from_target: counters.from_target.load(Ordering::Relaxed),
        stream_error,
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

/// Caller → destination; `end` or a clean caller half-close shuts down the TCP write side.
async fn upstream<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    mut target: OwnedWriteHalf,
    counters: &Counters,
) -> Result<(), Stop> {
    let ended = loop {
        let frame = match reader.next().await {
            Ok(Some(frame)) => frame,
            Ok(None) => break false,
            Err(error) => return Err(Stop::Caller(error)),
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    return Err(Stop::Caller(ServiceError::InvalidInput));
                };
                write(&mut target, bytes, counters).await?;
            }
            FrameKind::End if frame.end() == Ok(Channel::Input) => break true,
            FrameKind::Cancel => return Err(Stop::Caller(ServiceError::Cancelled)),
            _ => return Err(Stop::Caller(ServiceError::InvalidInput)),
        }
    };
    target.shutdown().await.map_err(|_| Stop::Target)?;
    if ended {
        // The caller still holds its stream open: a later cancel or reset must end the relay
        // at once rather than when a silent destination finally reaches the idle limit.
        let frame = tokio::select! {
            () = counters.delivered.notified() => return Ok(()),
            frame = reader.next() => frame,
        };
        return match frame {
            Ok(Some(frame)) if frame.kind() == FrameKind::Cancel => {
                Err(Stop::Caller(ServiceError::Cancelled))
            }
            Ok(Some(_)) => Err(Stop::Caller(ServiceError::InvalidInput)),
            // A later clean half-close changes nothing; the destination side decides the end.
            Ok(None) => Ok(()),
            Err(error) => Err(Stop::Caller(error)),
        };
    }
    Ok(())
}

/// Writes `bytes` to the destination. A write that stays blocked for the idle timeout ends
/// the relay even while the destination keeps sending, so one that never reads cannot hold it.
async fn write(target: &mut OwnedWriteHalf, bytes: &[u8], counters: &Counters) -> Result<(), Stop> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = tokio::time::timeout(counters.idle, target.write(rest))
            .await
            .map_err(|_| Stop::Idle)?
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

/// Destination → caller; destination EOF becomes `end`.
async fn downstream<W: AsyncWrite + Unpin>(
    mut target: OwnedReadHalf,
    writer: &mut FrameWriter<W>,
    counters: &Counters,
) -> Result<(), Stop> {
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = target.read(&mut buffer).await.map_err(|_| Stop::Target)?;
        if read == 0 {
            writer.end(Channel::Input).await.map_err(Stop::Caller)?;
            counters.delivered.notify_one();
            return Ok(());
        }
        let bytes = buffer.get(..read).unwrap_or_default();
        writer
            .data(Channel::Input, bytes)
            .await
            .map_err(Stop::Caller)?;
        counters
            .from_target
            .fetch_add(len64(read), Ordering::Relaxed);
        counters.touch();
    }
}
