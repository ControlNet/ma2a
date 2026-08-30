use ma2a_core::{EchoError, EchoResponse, EndpointId, MAX_WIRE_LEN};
use tokio::time::timeout;

use crate::ConnectionManager;

use super::{ECHO_ALPN, ECHO_DEADLINE, frame};

/// Cloneable single-attempt Echo client using the Runtime connection manager.
#[derive(Clone, Debug)]
pub struct EchoClient {
    connections: ConnectionManager,
}

impl EchoClient {
    pub(crate) const fn new(connections: ConnectionManager) -> Self {
        Self { connections }
    }

    /// Exchanges one canonical request with an exact target Endpoint.
    ///
    /// # Errors
    /// Returns a typed Echo failure for invalid input, timeout, transport, or response errors.
    pub async fn exchange(
        &self,
        target: EndpointId,
        request: &[u8],
    ) -> Result<EchoResponse<'static>, EchoError> {
        if request.len() > MAX_WIRE_LEN {
            return Err(EchoError::InvalidInput);
        }
        let response = timeout(ECHO_DEADLINE, self.exchange_inner(target, request))
            .await
            .map_err(|_| EchoError::TimedOut)??;
        let body = response.result?;
        if response.responder != target {
            return Err(EchoError::InvalidInput);
        }
        EchoResponse::decode(&body, response.responder, response.duration_ms)
            .map(EchoResponse::into_owned)
    }

    async fn exchange_inner(
        &self,
        target: EndpointId,
        request: &[u8],
    ) -> Result<frame::EchoFrame, EchoError> {
        let address = target.to_public_key().map_err(EchoError::from)?.into();
        let connection = self
            .connections
            .connect_addr(address, ECHO_ALPN)
            .await
            .map_err(|_| EchoError::Unavailable)?;
        let (mut send, mut receive) = connection
            .open_bi()
            .await
            .map_err(|_| EchoError::Unavailable)?;
        send.write_all(request)
            .await
            .map_err(|_| EchoError::Unavailable)?;
        send.finish().map_err(|_| EchoError::Unavailable)?;
        let response = frame::read(&mut receive).await;
        connection.close(0_u8.into(), b"");
        response
    }
}
