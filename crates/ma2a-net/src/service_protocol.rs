//! Transport for the closed set of built-in streaming services.
//!
//! The transport admits and bounds streams but never reads one: a call reaches the
//! Runtime with its stream unread, and the Runtime authorizes before reading the
//! bounded `open` frame and again before reading any content.

mod call;
mod client;
mod handler;
mod limit;

use ma2a_core::ServiceKind;

pub use call::{ServiceCall, ServiceMetrics, ServiceMetricsSnapshot, ServicePermit, ServiceStream};
pub use client::ServiceClient;
pub(crate) use handler::ServiceHandler;
pub use limit::ServiceLimits;

/// Exact Files v1 application ALPN.
pub const FILES_ALPN: &[u8] = b"ma2a/files/1";
/// Exact Transfer v1 application ALPN.
pub const TRANSFER_ALPN: &[u8] = b"ma2a/transfer/1";
/// Exact Exec v1 application ALPN.
pub const EXEC_ALPN: &[u8] = b"ma2a/exec/1";
/// Exact Shell v1 application ALPN.
pub const SHELL_ALPN: &[u8] = b"ma2a/shell/1";
/// Exact Proxy v1 application ALPN.
pub const PROXY_ALPN: &[u8] = b"ma2a/proxy/1";

/// Every streaming service in ALPN inventory order.
pub const STREAMING_SERVICES: [ServiceKind; 5] = [
    ServiceKind::Files,
    ServiceKind::Transfer,
    ServiceKind::Exec,
    ServiceKind::Shell,
    ServiceKind::Proxy,
];

/// Returns the exact ALPN of a streaming service, or `None` for Echo.
pub const fn service_alpn(kind: ServiceKind) -> Option<&'static [u8]> {
    match kind {
        ServiceKind::Echo => None,
        ServiceKind::Files => Some(FILES_ALPN),
        ServiceKind::Transfer => Some(TRANSFER_ALPN),
        ServiceKind::Exec => Some(EXEC_ALPN),
        ServiceKind::Shell => Some(SHELL_ALPN),
        ServiceKind::Proxy => Some(PROXY_ALPN),
    }
}
