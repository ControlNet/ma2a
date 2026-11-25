use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use ma2a_core::{
    EndpointId, ServiceKind,
    service::{FrameHeader, FrameKind, ServiceError},
};
use tokio::sync::Semaphore;

pub use super::limit::ServicePermit;

const REJECT_WRITE_DEADLINE: Duration = Duration::from_secs(2);

/// Admission counters used to prove that no stream byte is read before authorization.
#[derive(Clone, Debug, Default)]
pub struct ServiceMetrics(Arc<ServiceMetricCounters>);

#[derive(Debug, Default)]
struct ServiceMetricCounters {
    active: AtomicU64,
    authorized: AtomicU64,
    rejected: AtomicU64,
    saturated: AtomicU64,
}

/// Point-in-time streaming service admission counters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceMetricsSnapshot {
    /// Streams currently holding concurrency capacity.
    pub active_streams: u64,
    /// Streams handed to Runtime code after authorization.
    pub authorized_streams: u64,
    /// Streams rejected by the Runtime before any stream byte was read.
    pub rejected_streams: u64,
    /// Streams rejected by the transport because a concurrency bound was full.
    pub saturated_streams: u64,
}

impl ServiceMetrics {
    /// Returns current admission counters.
    pub fn snapshot(&self) -> ServiceMetricsSnapshot {
        ServiceMetricsSnapshot {
            active_streams: self.0.active.load(Ordering::Relaxed),
            authorized_streams: self.0.authorized.load(Ordering::Relaxed),
            rejected_streams: self.0.rejected.load(Ordering::Relaxed),
            saturated_streams: self.0.saturated.load(Ordering::Relaxed),
        }
    }

    pub(super) fn record_saturated(&self) {
        self.0.saturated.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct ActiveServiceStream(ServiceMetrics);

impl ActiveServiceStream {
    fn new(metrics: &ServiceMetrics) -> Self {
        metrics.0.active.fetch_add(1, Ordering::Relaxed);
        Self(metrics.clone())
    }
}

impl Drop for ActiveServiceStream {
    fn drop(&mut self) {
        self.0.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One admitted bidirectional stream and the capacity it holds until dropped.
///
/// The halves are only lent out, so the concurrency permit cannot be separated from them.
#[derive(Debug)]
pub struct ServiceStream {
    send: SendStream,
    receive: RecvStream,
    _connection: Option<Connection>,
    permit: Option<ServicePermit>,
    active: Option<ActiveServiceStream>,
}

impl ServiceStream {
    /// Lends both halves for concurrent reading and writing.
    pub const fn halves(&mut self) -> (&mut SendStream, &mut RecvStream) {
        (&mut self.send, &mut self.receive)
    }

    pub(super) const fn rejected(send: SendStream, receive: RecvStream) -> Self {
        Self {
            send,
            receive,
            _connection: None,
            permit: None,
            active: None,
        }
    }

    pub(super) const fn outbound(
        connection: Connection,
        send: SendStream,
        receive: RecvStream,
    ) -> Self {
        Self {
            send,
            receive,
            _connection: Some(connection),
            permit: None,
            active: None,
        }
    }
}

/// One TLS-authenticated inbound stream whose bytes stay unread until authorization.
#[derive(Debug)]
pub struct ServiceCall {
    remote_endpoint_id: EndpointId,
    kind: ServiceKind,
    stream: ServiceStream,
    metrics: ServiceMetrics,
    refusals: Arc<Semaphore>,
}

impl ServiceCall {
    pub(super) fn new(
        (remote_endpoint_id, kind): (EndpointId, ServiceKind),
        (send, receive): (SendStream, RecvStream),
        (permit, metrics, refusals): (ServicePermit, &ServiceMetrics, &Arc<Semaphore>),
    ) -> Self {
        Self {
            remote_endpoint_id,
            kind,
            stream: ServiceStream {
                send,
                receive,
                _connection: None,
                permit: Some(permit),
                active: Some(ActiveServiceStream::new(metrics)),
            },
            metrics: metrics.clone(),
            refusals: Arc::clone(refusals),
        }
    }

    /// Returns the identity authenticated by Iroh TLS.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }

    /// Returns the service selected by the negotiated ALPN.
    pub const fn kind(&self) -> ServiceKind {
        self.kind
    }

    /// Hands the unread stream to Runtime code after coarse admission succeeded.
    pub fn authorize(self) -> ServiceStream {
        self.metrics.0.authorized.fetch_add(1, Ordering::Relaxed);
        self.stream
    }

    /// Rejects the stream without reading it; the reject frame is written in the background,
    /// and the stream gives its concurrency place back first, so a refused peer holds none.
    pub fn reject(mut self, error: ServiceError) {
        self.metrics.0.rejected.fetch_add(1, Ordering::Relaxed);
        drop((self.stream.permit.take(), self.stream.active.take()));
        refuse(&self.refusals, self.stream, error);
    }
}

/// Writes a refused stream's reject frame in the background while fewer refusals of its
/// service than its Runtime-wide bound are being written, and otherwise resets the stream at
/// once with no reply, so a flood of refused streams cannot pile up writers.
pub(super) fn refuse(refusals: &Arc<Semaphore>, mut stream: ServiceStream, error: ServiceError) {
    let Ok(permit) = Arc::clone(refusals).try_acquire_owned() else {
        let _reset = stream.send.reset(0_u8.into());
        let _stopped = stream.receive.stop(0_u8.into());
        return;
    };
    tokio::spawn(async move {
        write_reject(stream, error).await;
        drop(permit);
    });
}

async fn write_reject(mut stream: ServiceStream, error: ServiceError) {
    let mut finished = false;
    let _written = tokio::time::timeout(REJECT_WRITE_DEADLINE, async {
        let header = FrameHeader::new(FrameKind::Reject, 1)
            .and_then(FrameHeader::encode)
            .ok()?;
        stream.send.write_all(&header).await.ok()?;
        stream.send.write_all(&[error.code()]).await.ok()?;
        stream.send.finish().ok()?;
        finished = true;
        let _unread = stream.receive.stop(0_u8.into());
        stream.send.stopped().await.ok()
    })
    .await;
    // Dropping an unfinished stream finishes it, so a frame cut short would reach the peer
    // as a clean end; it is reset instead. A finished reject is never reset, which could
    // discard it before the peer read it.
    if !finished {
        let _reset = stream.send.reset(0_u8.into());
    }
}
