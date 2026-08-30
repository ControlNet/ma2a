use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EchoError, EndpointId, MAX_WIRE_LEN};
use tokio::{sync::mpsc, task::JoinSet};

use super::{ECHO_DEADLINE, EchoCall, EchoServiceResponse, frame, limit::EchoLimiter};

const ECHO_RESPONSE_WRITE_BUDGET: Duration = Duration::from_millis(100);
const ECHO_PROCESS_DEADLINE: Duration = ECHO_DEADLINE.saturating_sub(ECHO_RESPONSE_WRITE_BUDGET);

/// Body-processing counters used to prove authorization precedes reads and decoding.
#[derive(Clone, Debug, Default)]
pub struct EchoMetrics(Arc<EchoMetricCounters>);

#[derive(Debug, Default)]
struct EchoMetricCounters {
    active_streams: AtomicU64,
    body_reads: AtomicU64,
    decoded_requests: AtomicU64,
}

/// Point-in-time Echo body-processing counters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EchoMetricsSnapshot {
    /// Number of currently admitted streams holding concurrency capacity.
    pub active_streams: u64,
    /// Number of request bodies read after successful admission.
    pub body_reads: u64,
    /// Number of canonical requests decoded by Runtime service dispatch.
    pub decoded_requests: u64,
}

impl EchoMetrics {
    /// Returns current body-processing counters.
    pub fn snapshot(&self) -> EchoMetricsSnapshot {
        EchoMetricsSnapshot {
            active_streams: self.0.active_streams.load(Ordering::Relaxed),
            body_reads: self.0.body_reads.load(Ordering::Relaxed),
            decoded_requests: self.0.decoded_requests.load(Ordering::Relaxed),
        }
    }

    /// Records one Runtime decode after transport admission and body read.
    pub fn record_decoded_request(&self) {
        self.0.decoded_requests.fetch_add(1, Ordering::Relaxed);
    }
}

struct ActiveEchoStream(EchoMetrics);

impl ActiveEchoStream {
    fn new(metrics: &EchoMetrics) -> Self {
        metrics.0.active_streams.fetch_add(1, Ordering::Relaxed);
        Self(metrics.clone())
    }
}

impl Drop for ActiveEchoStream {
    fn drop(&mut self) {
        self.0.0.active_streams.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EchoHandler {
    local_endpoint_id: EndpointId,
    calls: Option<mpsc::Sender<EchoCall>>,
    limiter: EchoLimiter,
    metrics: EchoMetrics,
}

impl EchoHandler {
    pub(crate) fn new(
        local_endpoint_id: EndpointId,
        calls: Option<mpsc::Sender<EchoCall>>,
        metrics: EchoMetrics,
    ) -> Self {
        Self {
            local_endpoint_id,
            calls,
            limiter: EchoLimiter::default(),
            metrics,
        }
    }
}

impl ProtocolHandler for EchoHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id().into();
        let mut streams = JoinSet::new();
        loop {
            tokio::select! {
                _ = connection.closed() => break,
                stream = connection.accept_bi() => match stream {
                    Ok((send, receive)) => {
                        let handler = self.clone();
                        streams.spawn(async move { handler.handle_stream(peer, send, receive).await });
                    }
                    Err(_) => break,
                },
                joined = streams.join_next(), if !streams.is_empty() => {
                    let _completed = joined;
                }
            }
        }
        streams.shutdown().await;
        Ok(())
    }
}

impl EchoHandler {
    #[expect(
        clippy::too_many_arguments,
        reason = "the private stream task owns handler, peer identity, and both QUIC halves"
    )]
    async fn handle_stream(
        self,
        peer: EndpointId,
        mut send: iroh::endpoint::SendStream,
        receive: iroh::endpoint::RecvStream,
    ) {
        let local_endpoint_id = self.local_endpoint_id;
        let stream_deadline = tokio::time::Instant::now() + ECHO_DEADLINE;
        let (duration_ms, response) =
            match tokio::time::timeout(ECHO_PROCESS_DEADLINE, self.process(peer, receive)).await {
                Ok(Ok(response)) => {
                    let (body, duration_ms) = response.into_parts();
                    (duration_ms, Ok(body))
                }
                Ok(Err(error)) => (0, Err(error)),
                Err(_) => (ma2a_core::MAX_ECHO_DURATION_MS, Err(EchoError::TimedOut)),
            };
        let _completed = tokio::time::timeout_at(
            stream_deadline,
            frame::write(&mut send, local_endpoint_id, duration_ms, response),
        )
        .await;
    }

    async fn process(
        &self,
        peer: EndpointId,
        mut receive: iroh::endpoint::RecvStream,
    ) -> Result<EchoServiceResponse, EchoError> {
        let _permit = self
            .limiter
            .try_acquire(peer)
            .ok_or(EchoError::ConcurrencyExceeded)?;
        let _active_stream = ActiveEchoStream::new(&self.metrics);
        let calls = self.calls.as_ref().ok_or(EchoError::Unavailable)?;
        let (call, admission, request, response) = EchoCall::channel(peer);
        calls.send(call).await.map_err(|_| EchoError::Unavailable)?;
        admission.await.map_err(|_| EchoError::Cancelled)??;
        let body = read_body(&mut receive).await?;
        self.metrics.0.body_reads.fetch_add(1, Ordering::Relaxed);
        request.send(body).map_err(|_| EchoError::Cancelled)?;
        response.await.map_err(|_| EchoError::Cancelled)?
    }
}

async fn read_body(receive: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>, EchoError> {
    let mut body = Vec::with_capacity(MAX_WIRE_LEN);
    let mut chunk = [0_u8; 512];
    loop {
        let Some(read) = receive
            .read(&mut chunk)
            .await
            .map_err(|_| EchoError::InvalidInput)?
        else {
            return Ok(body);
        };
        let next_len = body
            .len()
            .checked_add(read)
            .ok_or(EchoError::InvalidInput)?;
        if next_len > MAX_WIRE_LEN {
            return Err(EchoError::InvalidInput);
        }
        let bytes = chunk.get(..read).ok_or(EchoError::InvalidInput)?;
        body.extend_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests;
