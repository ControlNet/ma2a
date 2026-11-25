//! One bound on the inbound connections a peer holds open at once, across every ALPN.

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

use crate::peer_limit::PeerLimiter;

/// Inbound connections one peer may hold open at once, whatever their ALPN.
///
/// A peer that behaves needs at most 16 Echo calls, 8 enrollment exchanges and 2 control rounds at once,
/// each on its own connection, plus one cached connection per service family; the rest leaves
/// room for a dial that replaces a stale connection.
pub const PEER_CONNECTION_LIMIT: usize = 64;

/// Application close code of a connection refused before its handler read anything from it,
/// so whatever it carried had no effect and may be sent again later.
pub(crate) const BUSY_CLOSE_CODE: u8 = 3;

/// Counts every inbound connection against its peer from accept until its handler returns,
/// including any wait for the peer to close. There is no Runtime-wide connection bound.
#[derive(Clone, Debug)]
pub(crate) struct InboundLimiter(PeerLimiter);

impl Default for InboundLimiter {
    fn default() -> Self {
        Self(PeerLimiter::new(PEER_CONNECTION_LIMIT, usize::MAX))
    }
}

impl InboundLimiter {
    /// Wraps one ALPN's handler so its connections count against the shared bound.
    pub(crate) fn bound<H>(&self, handler: H) -> Bounded<H> {
        Bounded {
            handler,
            connections: self.0.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Bounded<H> {
    handler: H,
    connections: PeerLimiter,
}

impl<H: ProtocolHandler> ProtocolHandler for Bounded<H> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(_permit) = self.connections.try_acquire(connection.remote_id().into()) else {
            connection.close(BUSY_CLOSE_CODE.into(), b"");
            return Ok(());
        };
        self.handler.accept(connection).await
    }
}
