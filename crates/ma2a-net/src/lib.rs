//! Iroh transport adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod endpoint;
mod protocols;

pub use endpoint::{EndpointSecret, InvalidEndpointSecret, NetError, RuntimeEndpoint};
pub use iroh::EndpointAddr;
pub use protocols::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole};
