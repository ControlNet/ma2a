//! Encrypted bounded Echo v1 transport protocol.

mod call;
mod client;
mod frame;
mod handler;
mod limit;

use std::time::Duration;

pub use call::{EchoAuthorizedCall, EchoCall, EchoServiceResponse};
pub use client::EchoClient;
pub(crate) use handler::EchoHandler;
pub use handler::{EchoMetrics, EchoMetricsSnapshot};

/// Exact Echo v1 application ALPN.
pub const ECHO_ALPN: &[u8] = b"ma2a/echo/1";
/// Aggregate deadline for one Echo stream.
pub const ECHO_DEADLINE: Duration = Duration::from_secs(10);
/// Maximum concurrent Echo streams for one authenticated peer.
pub const ECHO_PER_PEER_LIMIT: usize = 16;
/// Maximum concurrent Echo streams across all peers.
pub const ECHO_GLOBAL_LIMIT: usize = 128;
