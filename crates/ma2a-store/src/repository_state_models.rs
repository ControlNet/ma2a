use ma2a_core::{EndpointId, SpaceId};

/// Closed Phase 1 membership role code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRole(u8);

impl MemberRole {
    /// Space owner role.
    pub const OWNER: Self = Self(0);
    /// Delegated administrator role.
    pub const ADMIN: Self = Self(1);
    /// Ordinary member role.
    pub const MEMBER: Self = Self(2);

    pub(crate) const fn code(self) -> u8 {
        self.0
    }
}

/// Current accepted membership at one manifest generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRecord {
    pub(crate) space_id: SpaceId,
    pub(crate) endpoint_id: EndpointId,
    pub(crate) role: MemberRole,
    pub(crate) accepted_generation: u64,
}

impl MemberRecord {
    /// Creates one current membership record.
    #[expect(
        clippy::too_many_arguments,
        reason = "the fixed membership key, role, and accepted generation are all required"
    )]
    pub const fn new(
        space_id: SpaceId,
        endpoint_id: EndpointId,
        role: MemberRole,
        accepted_generation: u64,
    ) -> Self {
        Self {
            space_id,
            endpoint_id,
            role,
            accepted_generation,
        }
    }
}

/// Signed Space-scoped revocation retained at its manifest generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRevocation {
    pub(crate) space_id: SpaceId,
    pub(crate) endpoint_id: EndpointId,
    pub(crate) revoked_generation: u64,
    pub(crate) signed_revocation: Vec<u8>,
}

impl MemberRevocation {
    /// Creates one signed revocation persistence record.
    #[expect(
        clippy::too_many_arguments,
        reason = "the fixed revocation record requires identity, generation, and signature bytes"
    )]
    pub const fn new(
        space_id: SpaceId,
        endpoint_id: EndpointId,
        revoked_generation: u64,
        signed_revocation: Vec<u8>,
    ) -> Self {
        Self {
            space_id,
            endpoint_id,
            revoked_generation,
            signed_revocation,
        }
    }
}

/// Runtime boot and shutdown metadata update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct RuntimeMetadataUpdate {
    /// Current boot identifier.
    pub boot_id: [u8; 16],
    /// Whether the previous shutdown was clean.
    pub last_shutdown_clean: bool,
    /// Observation timestamp for the shutdown state.
    pub observed_at_ms: i64,
}

/// Current persisted Endpoint observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "the complete schema-v1 Endpoint observation is written atomically"
)]
pub struct EndpointObservationUpdate {
    /// Observation timestamp.
    pub observed_at_ms: i64,
    /// Whether the Runtime has completed Endpoint startup.
    pub ready: bool,
    /// Number of observed direct addresses.
    pub direct_address_count: u64,
    /// Number of observed relay addresses.
    pub relay_address_count: u64,
    /// Number of locally valid Space memberships.
    pub membership_count: u64,
}

/// Persisted Runtime boot, shutdown, and Endpoint observation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeMetadata {
    pub(crate) revision: u64,
    pub(crate) boot_id: Option<[u8; 16]>,
    pub(crate) last_shutdown_clean: Option<bool>,
    pub(crate) last_shutdown_at_ms: Option<i64>,
    pub(crate) endpoint_observation: Option<EndpointObservationUpdate>,
}

impl RuntimeMetadata {
    /// Returns the monotonic Runtime state revision.
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Returns the current boot identifier when startup has begun.
    pub const fn boot_id(self) -> Option<[u8; 16]> {
        self.boot_id
    }

    /// Returns whether the current boot completed a clean shutdown.
    pub const fn last_shutdown_clean(self) -> Option<bool> {
        self.last_shutdown_clean
    }

    /// Returns the shutdown observation timestamp.
    pub const fn last_shutdown_at_ms(self) -> Option<i64> {
        self.last_shutdown_at_ms
    }

    /// Returns the latest Endpoint observation.
    pub const fn endpoint_observation(self) -> Option<EndpointObservationUpdate> {
        self.endpoint_observation
    }
}

/// Desired local public/private relay configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct RelayConfiguration {
    /// Enables separately configured public Iroh Relay fallback.
    pub public_fallback_enabled: bool,
    /// Optional public Relay URL.
    pub public_relay_url: Option<String>,
    /// Enables the local Runtime's private-relay provider role.
    pub private_provider_enabled: bool,
    /// Optional local private-relay listener address.
    pub listener_address: Option<String>,
    /// Native TLS or external termination mode code.
    pub tls_mode: Option<u8>,
}

/// Latest observed effective state for one relay URL.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct RelayObservation {
    /// Relay URL being observed.
    pub relay_url: String,
    /// Observation timestamp.
    pub observed_at_ms: i64,
    /// Observation expiry timestamp.
    pub expires_at_ms: i64,
    /// Whether the relay was reachable.
    pub reachable: bool,
    /// Optional measured latency.
    pub latency_ms: Option<u64>,
    /// Bounded runtime-owned observation payload.
    pub observed_state: Vec<u8>,
}
