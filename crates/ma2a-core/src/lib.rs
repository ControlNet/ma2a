//! Shared compile-time boundary for MA2A protocol and identity types.

#![forbid(unsafe_code)]

mod authorization;
mod codec;
mod enrollment_page;
mod error;
mod ids;
mod invite;
mod manifest;
mod manifest_inputs;
mod ontology;
mod policy;
mod space;
mod space_chain;
mod space_codec;
mod space_inputs;
mod wire;

pub use authorization::{SpaceAuthorizationView, authorize_any};
pub use codec::{decode_request, decode_response, encode_request, encode_response};
pub use enrollment_page::{
    EnrollmentPage, MAX_ENROLLMENT_ARTIFACTS_PER_PAGE, MAX_ENROLLMENT_PAGE_BYTES,
    validate_enrollment_pages,
};
pub use error::ProtocolError;
pub use ids::{EndpointId, RequestId, SpaceId};
pub use invite::{
    InviteEntropy, InviteValidity, MAX_INVITE_LIFETIME_MS, MAX_INVITE_TICKET_BYTES,
    SignedInviteTicket,
};
pub use manifest::{
    MANIFEST_HASH_DOMAIN, MANIFEST_SIGNATURE_DOMAIN, SignedSpaceManifestV1, SpaceManifestV1,
};
pub use manifest_inputs::{SpaceManifestLink, SpaceManifestMembership};
pub use ontology::{RuntimeIdentity, ServiceKind, SpaceMembership};
pub use policy::{
    Capability, MAX_MEMBER_LABEL_LEN, MAX_SPACE_MEMBERS, MemberCapabilities, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
};
pub use space::{
    GENESIS_CHAIN_HASH_DOMAIN, GENESIS_SIGNATURE_DOMAIN, SignedSpaceGenesisV1,
    SpaceAuthorityPublicKey, SpaceAuthoritySecret, SpaceGenesisV1,
};
pub use space_chain::{ManifestApplyOutcome, ManifestError, SpaceChain};
pub use space_inputs::{SpaceGenesisIdentity, SpaceGenesisOwner};
pub use wire::{
    MAX_PAYLOAD_LEN, MAX_WIRE_LEN, ProtocolVersion, RequestEnvelope, RequestOperation,
    ResponseEnvelope, ResponseResult,
};
