use iroh::{
    Endpoint, EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EndpointId, MAX_CONTROL_BATCH_BYTES};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::NetError;

/// Existing-member control synchronization ALPN.
pub const CONTROL_ALPN: &[u8] = b"ma2a/control/1";

const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Stable fail-closed response classification for an inbound control request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ControlRejection {
    /// The authenticated Endpoint is not a current shared-Space member.
    Unauthorized,
    /// The bounded request or response is malformed.
    Invalid,
    /// The local control service could not complete the request.
    Unavailable,
}

impl ControlRejection {
    const fn status(self) -> u8 {
        match self {
            Self::Unauthorized => 1,
            Self::Invalid => 2,
            Self::Unavailable => 255,
        }
    }
}

/// One TLS-authenticated bounded inbound control request.
#[derive(Debug)]
pub struct ControlCall {
    remote_endpoint_id: EndpointId,
    request: Vec<u8>,
    reply: oneshot::Sender<Result<Vec<u8>, ControlRejection>>,
}

impl ControlCall {
    /// Returns the Endpoint identity authenticated by Iroh TLS.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }

    /// Returns the bounded request bytes before application parsing.
    pub fn request(&self) -> &[u8] {
        &self.request
    }

    /// Completes the request with bounded bytes or a stable rejection.
    pub fn respond(self, response: Result<Vec<u8>, ControlRejection>) {
        let _unsent = self.reply.send(response);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ControlHandler {
    calls: Option<mpsc::Sender<ControlCall>>,
}

impl ControlHandler {
    pub(crate) const fn new(calls: Option<mpsc::Sender<ControlCall>>) -> Self {
        Self { calls }
    }
}

impl ProtocolHandler for ControlHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(calls) = &self.calls else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        let remote_endpoint_id = connection.remote_id().into();
        let Ok(stream) = timeout(CONTROL_IO_TIMEOUT, connection.accept_bi()).await else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        let (mut send, mut receive) = stream?;
        let request = timeout(
            CONTROL_IO_TIMEOUT,
            receive.read_to_end(MAX_CONTROL_BATCH_BYTES),
        )
        .await
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)?;
        let (reply, response) = oneshot::channel();
        if timeout(
            CONTROL_IO_TIMEOUT,
            calls.send(ControlCall {
                remote_endpoint_id,
                request,
                reply,
            }),
        )
        .await
        .map_or(true, |result| result.is_err())
        {
            connection.close(1_u8.into(), b"");
            return Ok(());
        }
        let result = timeout(CONTROL_IO_TIMEOUT, response)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(Err(ControlRejection::Unavailable));
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
        let _closed = timeout(CONTROL_IO_TIMEOUT, connection.closed()).await;
        Ok(())
    }
}

/// Cloneable active-dial client backed by the running Runtime Endpoint.
#[derive(Clone, Debug)]
pub struct ControlClient {
    endpoint: Endpoint,
}

impl ControlClient {
    pub(crate) const fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// Exchanges one bounded request with an exact resolved Endpoint address.
    ///
    /// # Errors
    /// Returns [`NetError`] for connection, framing, timeout, or peer rejection.
    pub async fn exchange_addr(
        &self,
        target: EndpointAddr,
        request: &[u8],
    ) -> Result<Vec<u8>, NetError> {
        self.exchange(target, request).await
    }

    /// Resolves and actively dials one Endpoint through the configured private lookup.
    ///
    /// # Errors
    /// Returns [`NetError`] for resolution, connection, framing, timeout, or peer rejection.
    pub async fn exchange_endpoint(
        &self,
        target: EndpointId,
        request: &[u8],
    ) -> Result<Vec<u8>, NetError> {
        let public_key = target
            .to_public_key()
            .map_err(|_| NetError::control_permanent())?;
        self.exchange(public_key, request).await
    }

    async fn exchange(
        &self,
        target: impl Into<EndpointAddr>,
        request: &[u8],
    ) -> Result<Vec<u8>, NetError> {
        if request.len() > MAX_CONTROL_BATCH_BYTES {
            return Err(NetError::control_permanent());
        }
        let connection = timeout(
            CONTROL_IO_TIMEOUT,
            self.endpoint.connect(target, CONTROL_ALPN),
        )
        .await
        .map_err(|_| NetError::control_transient())?
        .map_err(|_| NetError::control_transient())?;
        let (mut send, mut receive) = timeout(CONTROL_IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| NetError::control_transient())?
            .map_err(|_| NetError::control_transient())?;
        timeout(CONTROL_IO_TIMEOUT, send.write_all(request))
            .await
            .map_err(|_| NetError::control_transient())?
            .map_err(|_| NetError::control_transient())?;
        send.finish().map_err(|_| NetError::control_transient())?;
        let mut status = [0_u8; 1];
        timeout(CONTROL_IO_TIMEOUT, receive.read_exact(&mut status))
            .await
            .map_err(|_| NetError::control_transient())?
            .map_err(|_| NetError::control_transient())?;
        let mut length = [0_u8; 4];
        timeout(CONTROL_IO_TIMEOUT, receive.read_exact(&mut length))
            .await
            .map_err(|_| NetError::control_transient())?
            .map_err(|_| NetError::control_transient())?;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| NetError::control_permanent())?;
        match status[0] {
            0 if length <= MAX_CONTROL_BATCH_BYTES => {}
            255 => return Err(NetError::control_transient()),
            _ => return Err(NetError::control_permanent()),
        }
        let mut response = vec![0_u8; length];
        timeout(CONTROL_IO_TIMEOUT, receive.read_exact(&mut response))
            .await
            .map_err(|_| NetError::control_transient())?
            .map_err(|_| NetError::control_transient())?;
        connection.close(0_u8.into(), b"");
        Ok(response)
    }
}
