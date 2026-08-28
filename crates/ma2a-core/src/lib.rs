//! Shared compile-time boundary for MA2A protocol and identity types.

#![forbid(unsafe_code)]

mod codec;
mod error;
mod ids;
mod ontology;
mod wire;

pub use codec::{decode_request, decode_response, encode_request, encode_response};
pub use error::ProtocolError;
pub use ids::{EndpointId, RequestId, SpaceId};
pub use ontology::{RuntimeIdentity, ServiceKind, SpaceMembership};
pub use wire::{
    MAX_PAYLOAD_LEN, MAX_WIRE_LEN, ProtocolVersion, RequestEnvelope, RequestOperation,
    ResponseEnvelope, ResponseResult,
};
