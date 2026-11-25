use ma2a_core::ServiceKind;

/// RAII capacity held for the whole life of one admitted stream.
pub use crate::peer_limit::PeerPermit as ServicePermit;

/// Per-peer and Runtime-wide concurrent stream bounds of one service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceLimits {
    per_peer: usize,
    global: usize,
}

impl ServiceLimits {
    /// Returns the fixed bounds of a streaming service, from the service catalog.
    pub const fn of(kind: ServiceKind) -> Self {
        let (per_peer, global) = ma2a_core::service::stream_limits(kind);
        Self { per_peer, global }
    }

    /// Returns the per-peer bound.
    pub const fn per_peer(self) -> usize {
        self.per_peer
    }

    /// Returns the Runtime-wide bound.
    pub const fn global(self) -> usize {
        self.global
    }
}
