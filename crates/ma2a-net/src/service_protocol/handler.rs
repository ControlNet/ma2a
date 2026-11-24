use std::sync::Arc;

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EndpointId, ServiceKind, service::ServiceError};
use tokio::sync::{Semaphore, mpsc};

use super::{
    ServiceCall, ServiceLimits, ServiceMetrics,
    call::{ServiceStream, refuse},
    limit::ServiceLimiter,
};

/// Inbound handler for one streaming service ALPN.
#[derive(Clone, Debug)]
pub(crate) struct ServiceHandler {
    kind: ServiceKind,
    calls: Option<mpsc::Sender<ServiceCall>>,
    limiter: ServiceLimiter,
    metrics: ServiceMetrics,
    /// Bounds the reject frames being written for this service across all its connections.
    refusals: Arc<Semaphore>,
}

impl ServiceHandler {
    pub(crate) fn new(
        kind: ServiceKind,
        calls: Option<mpsc::Sender<ServiceCall>>,
        metrics: ServiceMetrics,
    ) -> Self {
        let limits = ServiceLimits::of(kind);
        Self {
            kind,
            calls,
            limiter: ServiceLimiter::new(limits),
            metrics,
            refusals: Arc::new(Semaphore::new(limits.global())),
        }
    }
}

impl ProtocolHandler for ServiceHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer: EndpointId = connection.remote_id().into();
        loop {
            tokio::select! {
                _ = connection.closed() => break,
                stream = connection.accept_bi() => match stream {
                    Ok(halves) => {
                        if let Some(stream) = self.admit(peer, halves) {
                            refuse(&self.refusals, stream, ServiceError::ConcurrencyExceeded);
                        }
                    }
                    Err(_) => break,
                },
            }
        }
        Ok(())
    }
}

impl ServiceHandler {
    /// Queues an admitted call, or returns the unread stream that must be rejected.
    fn admit(
        &self,
        peer: EndpointId,
        (send, receive): (iroh::endpoint::SendStream, iroh::endpoint::RecvStream),
    ) -> Option<ServiceStream> {
        let Some(permit) = self.limiter.try_acquire(peer) else {
            self.metrics.record_saturated();
            return Some(ServiceStream::rejected(send, receive));
        };
        let call = ServiceCall::new(
            (peer, self.kind),
            (send, receive),
            (permit, &self.metrics, &self.refusals),
        );
        let Some(calls) = self.calls.as_ref() else {
            call.reject(ServiceError::Unavailable);
            return None;
        };
        if let Err(
            mpsc::error::TrySendError::Full(call) | mpsc::error::TrySendError::Closed(call),
        ) = calls.try_send(call)
        {
            call.reject(ServiceError::Unavailable);
        }
        None
    }
}

#[cfg(test)]
mod tests;
