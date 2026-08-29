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
    /// Ordered operator-supplied public Relay URLs.
    pub public_relay_urls: Vec<String>,
    /// Enables the local Runtime's private-relay provider role.
    pub private_provider_enabled: bool,
    /// Optional local private-relay listener address.
    pub listener_address: Option<String>,
    /// Externally advertised private Relay URL.
    pub private_relay_url: Option<String>,
    /// Exact configured Space subset served by the provider.
    pub served_spaces: Vec<SpaceId>,
    /// Operator-selected private Relay transport configuration.
    pub transport: Option<RelayTransportConfiguration>,
}

/// Persisted private Relay TLS deployment mode and path references.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Store owns the complete closed relay transport persistence contract"
)]
pub enum RelayTransportConfiguration {
    /// The embedded relay loads certificate and key files by path.
    NativeTls {
        /// Certificate chain path, never certificate bytes.
        certificate_path: String,
        /// Private-key path, never private-key bytes.
        private_key_path: String,
    },
    /// A loopback proxy backend receives externally terminated TLS traffic.
    ExternalTlsTermination,
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
use ma2a_core::SpaceId;
