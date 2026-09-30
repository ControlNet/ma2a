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
}

struct Counters {
    to_target: AtomicU64,
    from_target: AtomicU64,
    last_activity: AtomicU64,
    started: Instant,
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
        () = idle_expired(&counters, idle) => (Ok(EndReason::TimedOut), None),
        joined = both => match joined {
            Ok(((), ())) => (Ok(EndReason::Completed), None),
            Err(Stop::Caller(ServiceError::Cancelled)) => (Ok(EndReason::Cancelled), None),
            Err(Stop::Caller(error)) => (Ok(EndReason::Cancelled), Some(error)),
            Err(Stop::Target) => (Err(ServiceError::Unreachable), None),
        },
    };
    Relayed {
        result,
        to_target: counters.to_target.load(Ordering::Relaxed),
        from_target: counters.from_target.load(Ordering::Relaxed),
        stream_error,
    }
}

async fn idle_expired(counters: &Counters, idle: Duration) {
    let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
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
    loop {
        let frame = match reader.next().await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => return Err(Stop::Caller(error)),
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    return Err(Stop::Caller(ServiceError::InvalidInput));
                };
                target.write_all(bytes).await.map_err(|_| Stop::Target)?;
                counters
                    .to_target
                    .fetch_add(len64(bytes.len()), Ordering::Relaxed);
                counters.touch();
            }
            FrameKind::End if frame.end() == Ok(Channel::Input) => break,
            FrameKind::Cancel => return Err(Stop::Caller(ServiceError::Cancelled)),
            _ => return Err(Stop::Caller(ServiceError::InvalidInput)),
        }
    }
    target.shutdown().await.map_err(|_| Stop::Target)
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
