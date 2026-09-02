use std::{error::Error, fmt};

use crate::{Capability, EndpointId, SpaceChain, SpaceId, SpaceMemberV1, SpacePolicyV1};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Immutable authorization state derived from one verified Space chain.
pub struct SpaceAuthorizationView {
    space_id: SpaceId,
    generation: u64,
    manifest_hash: [u8; 32],
    policy: SpacePolicyV1,
    members: Vec<SpaceMemberV1>,
}

impl SpaceAuthorizationView {
    /// Builds an authorization view from the latest state of `chain`.
    pub fn from_chain(chain: &SpaceChain) -> Self {
        Self {
            space_id: chain.space_id(),
            generation: chain.latest_generation(),
            manifest_hash: chain.latest_hash(),
            policy: chain.genesis().genesis().policy(),
            members: chain.members().to_vec(),
        }
    }

    /// Returns whether the endpoint has the capability under both Space policy and membership.
    pub fn allows(&self, endpoint_id: EndpointId, capability: Capability) -> bool {
        self.policy.allows(capability)
            && self
                .members
                .binary_search_by_key(&endpoint_id, SpaceMemberV1::endpoint_id)
                .is_ok_and(|index| {
                    self.members
                        .get(index)
                        .is_some_and(|member| member.capabilities().allows(capability))
                })
    }

    /// Returns whether the Endpoint is a current member of this exact Space.
    pub fn contains_member(&self, endpoint_id: EndpointId) -> bool {
        self.members
            .binary_search_by_key(&endpoint_id, SpaceMemberV1::endpoint_id)
            .is_ok()
    }

    /// Returns whether Space policy permits a current member to provide private relay service.
    pub fn allows_private_relay_provider(&self, endpoint_id: EndpointId) -> bool {
        self.policy.allows(Capability::PRIVATE_RELAY_PROVIDER) && self.contains_member(endpoint_id)
    }

    /// Returns current unrevoked member Endpoint identities in canonical order.
    pub fn member_endpoint_ids(&self) -> impl Iterator<Item = EndpointId> + '_ {
        self.members.iter().map(SpaceMemberV1::endpoint_id)
    }

    /// Returns the Space identifier represented by this view.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns the manifest generation represented by this view.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the latest verified chain hash represented by this view.
    pub const fn manifest_hash(&self) -> [u8; 32] {
        self.manifest_hash
    }

    fn permits(&self, request: &AuthorizationRequest) -> bool {
        let endpoints = request.endpoints;
        if !self.contains_member(endpoints.caller) || !self.contains_member(endpoints.target) {
            return false;
        }
        if !request.resource_applies_to(self.space_id) {
            return false;
        }
        match request.operation.0 {
            RemoteOperationKind::EchoCall => {
                self.allows(endpoints.caller, Capability::ECHO)
                    && self.allows(endpoints.target, Capability::ECHO)
            }
            RemoteOperationKind::ControlSync
            | RemoteOperationKind::MetadataRead
            | RemoteOperationKind::AddressRecordExchange => true,
            RemoteOperationKind::RelayAdvertisement => {
                self.allows_private_relay_provider(endpoints.caller)
            }
            RemoteOperationKind::Unsupported => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteOperationKind {
    EchoCall,
    ControlSync,
    MetadataRead,
    AddressRecordExchange,
    RelayAdvertisement,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Closed normal remote operation set used before service-body parsing.
pub struct RemoteOperation(RemoteOperationKind);

impl RemoteOperation {
    /// Bounded Echo request-response operation.
    pub const ECHO_CALL: Self = Self(RemoteOperationKind::EchoCall);
    /// Existing-member control synchronization operation.
    pub const CONTROL_SYNC: Self = Self(RemoteOperationKind::ControlSync);
    /// Existing-member metadata read operation.
    pub const METADATA_READ: Self = Self(RemoteOperationKind::MetadataRead);
    /// Existing-member signed address-record exchange operation.
    pub const ADDRESS_RECORD_EXCHANGE: Self = Self(RemoteOperationKind::AddressRecordExchange);
    /// Private relay advertisement publication operation.
    pub const RELAY_ADVERTISEMENT: Self = Self(RemoteOperationKind::RelayAdvertisement);
    /// Fail-closed value for unsupported or future services.
    pub const UNSUPPORTED: Self = Self(RemoteOperationKind::Unsupported);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Authenticated remote caller and local target Endpoint identities.
pub struct AuthorizationEndpoints {
    caller: EndpointId,
    target: EndpointId,
}

impl AuthorizationEndpoints {
    /// Creates an Endpoint-centric authorization pair without a Space selector.
    pub const fn new(caller: EndpointId, target: EndpointId) -> Self {
        Self { caller, target }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthorizationResourceKind {
    ControlSpaceCursor(SpaceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Optional typed resource that can narrow an already-derived shared Space.
pub struct AuthorizationResource(AuthorizationResourceKind);

impl AuthorizationResource {
    /// Creates a control cursor that never independently grants Space access.
    pub const fn control_space_cursor(space_id: SpaceId) -> Self {
        Self(AuthorizationResourceKind::ControlSpaceCursor(space_id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Complete input to one endpoint-centric remote authorization decision.
pub struct AuthorizationRequest {
    endpoints: AuthorizationEndpoints,
    operation: RemoteOperation,
    resource: Option<AuthorizationResource>,
}

impl AuthorizationRequest {
    /// Creates one request without accepting an authoritative Space selector.
    pub const fn new(
        endpoints: AuthorizationEndpoints,
        operation: RemoteOperation,
        resource: Option<AuthorizationResource>,
    ) -> Self {
        Self {
            endpoints,
            operation,
            resource,
        }
    }

    /// Returns the closed operation for protocol routing.
    pub const fn operation(&self) -> RemoteOperation {
        self.operation
    }

    fn resource_applies_to(&self, shared_space: SpaceId) -> bool {
        match (self.operation.0, self.resource.map(|resource| resource.0)) {
            (
                RemoteOperationKind::ControlSync,
                Some(AuthorizationResourceKind::ControlSpaceCursor(cursor)),
            ) => cursor == shared_space,
            (
                RemoteOperationKind::ControlSync
                | RemoteOperationKind::EchoCall
                | RemoteOperationKind::MetadataRead
                | RemoteOperationKind::AddressRecordExchange
                | RemoteOperationKind::RelayAdvertisement
                | RemoteOperationKind::Unsupported,
                None,
            ) => true,
            (
                RemoteOperationKind::EchoCall
                | RemoteOperationKind::MetadataRead
                | RemoteOperationKind::AddressRecordExchange
                | RemoteOperationKind::RelayAdvertisement
                | RemoteOperationKind::Unsupported,
                Some(AuthorizationResourceKind::ControlSpaceCursor(_)),
            ) => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthorizationDeniedKind {
    AccessDenied,
}

#[derive(Clone, Copy, PartialEq, Eq)]
/// Stable non-leaking denial returned for every authorization failure.
pub struct AuthorizationDenied(AuthorizationDeniedKind);

impl AuthorizationDenied {
    /// The sole externally distinguishable denial.
    pub const ACCESS_DENIED: Self = Self(AuthorizationDeniedKind::AccessDenied);
}

impl fmt::Debug for AuthorizationDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizationDenied")
    }
}

impl fmt::Display for AuthorizationDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote operation is not authorized")
    }
}

impl Error for AuthorizationDenied {}

/// Opaque proof that one complete independently evaluated Space allowed the operation.
pub struct AuthorizationPermit {
    authorized_via: SpaceId,
    generation: u64,
    manifest_hash: [u8; 32],
}

impl AuthorizationPermit {
    fn matches(&self, space: &SpaceAuthorizationView) -> bool {
        self.authorized_via == space.space_id()
            && self.generation == space.generation()
            && self.manifest_hash == space.manifest_hash()
    }
}

impl fmt::Debug for AuthorizationPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizationPermit(..)")
    }
}

/// Evaluates each verified Space independently and returns the first complete allow.
///
/// # Errors
/// Returns the same [`AuthorizationDenied`] for every missing, revoked, partial, or unsupported grant.
pub fn authorize_endpoint(
    request: &AuthorizationRequest,
    spaces: &[SpaceAuthorizationView],
) -> Result<AuthorizationPermit, AuthorizationDenied> {
    spaces
        .iter()
        .find(|space| space.permits(request))
        .map(|space| {
            let permit = AuthorizationPermit {
                authorized_via: space.space_id,
                generation: space.generation,
                manifest_hash: space.manifest_hash,
            };
            debug_assert!(permit.matches(space));
            permit
        })
        .ok_or(AuthorizationDenied::ACCESS_DENIED)
}

/// Returns whether any Space grants the endpoint the requested capability.
pub fn authorize_any(
    spaces: &[SpaceAuthorizationView],
    endpoint_id: EndpointId,
    capability: Capability,
) -> bool {
    spaces
        .iter()
        .any(|space| space.allows(endpoint_id, capability))
}
