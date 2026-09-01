use iroh::EndpointAddr;
use ma2a_core::{EndpointId, MAX_CONTROL_BATCH_BYTES};
use tokio::time::{Duration, timeout};

use crate::{ConnectionManager, NetError};

/// Existing-member control synchronization ALPN.
pub const CONTROL_ALPN: &[u8] = b"ma2a/control/1";

pub(super) const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub(super) const fn status(self) -> u8 {
        match self {
            Self::Unauthorized => 1,
            Self::Invalid => 2,
            Self::Unavailable => 255,
        }
    }
}

/// Cloneable active-dial client backed by the running Runtime Endpoint.
#[derive(Clone, Debug)]
pub struct ControlClient {
    connections: ConnectionManager,
}

impl ControlClient {
    pub(crate) const fn new(connections: ConnectionManager) -> Self {
        Self { connections }
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
        let target = target.into();
        let connection = timeout(
            CONTROL_IO_TIMEOUT,
            self.connections.connect_addr(target, CONTROL_ALPN),
        )
        .await
        .map_err(|_| NetError::control_transient())?
        .map_err(|error| NetError::control_from_dial(&error))?;
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
