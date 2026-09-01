use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use iroh::{
    endpoint::{Connection, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::MAX_CONTROL_BATCH_BYTES;
use tokio::{sync::mpsc, time::timeout};

use super::{ControlCall, ControlRejection, limit::ControlLimiter, protocol::CONTROL_IO_TIMEOUT};

/// Body-processing counters used to prove control authorization precedes reads.
#[derive(Clone, Debug, Default)]
pub struct ControlMetrics(Arc<ControlMetricCounters>);

#[derive(Debug, Default)]
struct ControlMetricCounters {
    active_streams: AtomicU64,
    body_reads: AtomicU64,
}

/// Point-in-time inbound control admission counters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlMetricsSnapshot {
    /// Number of streams currently holding global and per-peer capacity.
    pub active_streams: u64,
    /// Number of complete request bodies read after Runtime authorization.
    pub body_reads: u64,
}

impl ControlMetrics {
    /// Returns current control admission counters.
    pub fn snapshot(&self) -> ControlMetricsSnapshot {
        ControlMetricsSnapshot {
            active_streams: self.0.active_streams.load(Ordering::Relaxed),
            body_reads: self.0.body_reads.load(Ordering::Relaxed),
        }
    }
}

struct ActiveControlStream(ControlMetrics);

impl ActiveControlStream {
    fn new(metrics: &ControlMetrics) -> Self {
        metrics.0.active_streams.fetch_add(1, Ordering::Relaxed);
        Self(metrics.clone())
    }
}

impl Drop for ActiveControlStream {
    fn drop(&mut self) {
        self.0.0.active_streams.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ControlHandler {
    calls: Option<mpsc::Sender<ControlCall>>,
    limiter: ControlLimiter,
    metrics: ControlMetrics,
}

impl ControlHandler {
    pub(crate) fn new(calls: Option<mpsc::Sender<ControlCall>>, metrics: ControlMetrics) -> Self {
        Self {
            calls,
            limiter: ControlLimiter::default(),
            metrics,
        }
    }
}

impl ProtocolHandler for ControlHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(calls) = &self.calls else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        let peer = connection.remote_id().into();
        let Ok(stream) = timeout(CONTROL_IO_TIMEOUT, connection.accept_bi()).await else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        let (mut send, mut receive) = stream?;
        let Some(_permit) = self.limiter.try_acquire(peer) else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        let _active_stream = ActiveControlStream::new(&self.metrics);
        let (call, admission, request, response) = ControlCall::channel(peer);
        if timeout(CONTROL_IO_TIMEOUT, calls.send(call))
            .await
            .map_or(true, |result| result.is_err())
        {
            connection.close(1_u8.into(), b"");
            return Ok(());
        }
        let result = match timeout(CONTROL_IO_TIMEOUT, admission).await {
            Ok(Ok(Ok(()))) => {
                let body = timeout(
                    CONTROL_IO_TIMEOUT,
                    receive.read_to_end(MAX_CONTROL_BATCH_BYTES),
                )
                .await
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)?;
                self.metrics.0.body_reads.fetch_add(1, Ordering::Relaxed);
                if request.send(body).is_err() {
                    Err(ControlRejection::Unavailable)
                } else {
                    timeout(CONTROL_IO_TIMEOUT, response)
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .unwrap_or(Err(ControlRejection::Unavailable))
                }
            }
            Ok(Ok(Err(rejection))) => Err(rejection),
            Ok(Err(_)) | Err(_) => Err(ControlRejection::Unavailable),
        };
        write_response(&mut send, result).await?;
        let _closed = timeout(CONTROL_IO_TIMEOUT, connection.closed()).await;
        Ok(())
    }
}

async fn write_response(
    send: &mut SendStream,
    result: Result<Vec<u8>, ControlRejection>,
) -> Result<(), AcceptError> {
    let (status, body) = match result {
        Ok(body) if body.len() <= MAX_CONTROL_BATCH_BYTES => (0, body),
        Ok(_) => (ControlRejection::Invalid.status(), Vec::new()),
        Err(rejection) => (rejection.status(), Vec::new()),
    };
    send.write_all(&[status])
        .await
        .map_err(std::io::Error::other)?;
    let length = u32::try_from(body.len()).map_err(std::io::Error::other)?;
    send.write_all(&length.to_be_bytes())
        .await
        .map_err(std::io::Error::other)?;
    send.write_all(&body).await.map_err(std::io::Error::other)?;
    send.finish().map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests;
