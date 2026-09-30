use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EndpointId, ServiceKind, service::ServiceError};
use tokio::{sync::mpsc, task::JoinSet};

use super::{
    ServiceCall, ServiceLimits, ServiceMetrics,
    call::{ServiceStream, write_reject},
    limit::ServiceLimiter,
};

/// Inbound handler for one streaming service ALPN.
#[derive(Clone, Debug)]
pub(crate) struct ServiceHandler {
    kind: ServiceKind,
    calls: Option<mpsc::Sender<ServiceCall>>,
    limiter: ServiceLimiter,
    metrics: ServiceMetrics,
}

impl ServiceHandler {
    pub(crate) fn new(
        kind: ServiceKind,
        calls: Option<mpsc::Sender<ServiceCall>>,
        metrics: ServiceMetrics,
    ) -> Self {
        Self {
            kind,
            calls,
            limiter: ServiceLimiter::new(ServiceLimits::of(kind)),
            metrics,
        }
    }
}

impl ProtocolHandler for ServiceHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer: EndpointId = connection.remote_id().into();
        let mut rejects = JoinSet::new();
        loop {
            tokio::select! {
                _ = connection.closed() => break,
                stream = connection.accept_bi() => match stream {
                    Ok(halves) => {
                        if let Some(stream) = self.admit(peer, halves) {
                            rejects.spawn(write_reject(stream, ServiceError::ConcurrencyExceeded));
                        }
                    }
                    Err(_) => break,
                },
                joined = rejects.join_next(), if !rejects.is_empty() => {
                    let _completed = joined;
                }
            }
        }
        rejects.shutdown().await;
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
        let call = ServiceCall::new((peer, self.kind), (send, receive), (permit, &self.metrics));
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
