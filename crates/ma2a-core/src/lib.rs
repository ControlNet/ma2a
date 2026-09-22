//! Shared compile-time boundary for MA2A protocol and identity types.

#![forbid(unsafe_code)]

mod address;
mod authorization;
mod codec;
mod control;
mod echo;
mod enrollment_bootstrap;
mod enrollment_page;
mod error;
mod ids;
mod invite;
mod manifest;
mod manifest_inputs;
mod ontology;
mod policy;
mod reachability;
mod relay;
mod space;
mod space_chain;
mod space_codec;
mod space_inputs;
mod wire;

pub use address::{
    ADDRESS_RECORD_HASH_DOMAIN, ADDRESS_SIGNATURE_DOMAIN, AddressEndpointDataV1,
    AddressRecordScope, AddressRecordValidity, MAX_ADDRESS_RECORD_ADDRESSES,
    MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN, MAX_ADDRESS_RECORD_LEN, MAX_ADDRESS_RECORD_USER_DATA_LEN,
    MAX_ADDRESS_RECORD_VALIDITY_MS, SignedSpaceAddressRecordV1, SpaceAddressRecordV1,
};
pub use authorization::{
    AuthorizationDenied, AuthorizationEndpoints, AuthorizationPermit, AuthorizationRequest,
    AuthorizationResource, RemoteOperation, SpaceAuthorizationView, authorize_any,
    authorize_endpoint,
};
pub use codec::{decode_request, decode_response, encode_request, encode_response};
pub use control::{
    ControlArtifactKind, ControlArtifactV1, ControlCursorEntryV1, ControlCursorV1, ControlPageV1,
    ControlRequestV1, ControlResponseV1, MAX_CONTROL_ARTIFACTS_PER_PAGE, MAX_CONTROL_BATCH_BYTES,
    MAX_CONTROL_CURSOR_ENTRIES, MAX_CONTROL_SPACES_PER_REQUEST,
};
pub use echo::{
    EchoError, EchoRequest, EchoResponse, EchoResultClass, EchoStatus, MAX_ECHO_DURATION_MS,
    MAX_ECHO_PAYLOAD_LEN,
};
pub use enrollment_bootstrap::{EnrollmentBootstrap, MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES};
pub use enrollment_page::{
    EnrollmentPage, MAX_ENROLLMENT_ARTIFACTS_PER_PAGE, MAX_ENROLLMENT_PAGE_BYTES,
    MAX_ENROLLMENT_PAGES, validate_enrollment_pages,
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
    SpacePolicyV1, SpaceRevocationV1, default_member_label,
};
pub use reachability::RelayReachability;
pub use relay::{
    MAX_PRIVATE_RELAY_ADVERTISEMENT_LEN, MAX_PRIVATE_RELAY_ADVERTISEMENT_VALIDITY_MS,
    MAX_PRIVATE_RELAY_URL_LEN, PRIVATE_RELAY_ADVERTISEMENT_HASH_DOMAIN,
    PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN, PrivateRelayAdvertisementScope,
    PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
    SignedPrivateRelayAdvertisementV1,
};
pub use space::{
    GENESIS_CHAIN_HASH_DOMAIN, GENESIS_SIGNATURE_DOMAIN, MAX_SPACE_NAME_LEN, SignedSpaceGenesisV1,
    SpaceAuthorityPublicKey, SpaceAuthoritySecret, SpaceGenesisV1,
};
pub use space_chain::{ManifestApplyOutcome, ManifestError, SpaceChain};
pub use space_inputs::{SpaceGenesisIdentity, SpaceGenesisOwner};
pub use wire::{
    MAX_PAYLOAD_LEN, MAX_WIRE_LEN, ProtocolVersion, RequestEnvelope, RequestOperation,
    ResponseEnvelope, ResponseResult,
};
