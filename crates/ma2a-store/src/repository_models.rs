use ma2a_core::{EndpointId, SpaceId};

use crate::KeyReference;

/// The Runtime's public Endpoint identity and protected-key reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRecord {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) key_reference: KeyReference,
}

impl EndpointRecord {
    /// Creates one persistent Endpoint record.
    pub const fn new(endpoint_id: EndpointId, key_reference: KeyReference) -> Self {
        Self {
            endpoint_id,
            key_reference,
        }
    }

    /// Returns the persisted public Endpoint identity.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }

    /// Returns the opaque protected-key reference.
    pub const fn key_reference(&self) -> &KeyReference {
        &self.key_reference
    }
}

/// Space genesis and an optional local authority-key reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceRecord {
    pub(crate) space_id: SpaceId,
    pub(crate) genesis_cbor: Vec<u8>,
    pub(crate) authority_key_reference: Option<KeyReference>,
}

impl SpaceRecord {
    /// Creates one Space persistence record.
    pub const fn new(
        space_id: SpaceId,
        genesis_cbor: Vec<u8>,
        authority_key_reference: Option<KeyReference>,
    ) -> Self {
        Self {
            space_id,
            genesis_cbor,
            authority_key_reference,
        }
    }
}

/// One pending invitation containing only a non-recoverable token hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvitationRecord {
    pub(crate) invitation_id: [u8; 16],
    pub(crate) space_id: SpaceId,
    pub(crate) token_hash: [u8; 32],
    pub(crate) expires_at_ms: i64,
}

impl InvitationRecord {
    /// Creates a pending invitation record.
    #[expect(
        clippy::too_many_arguments,
        reason = "the fixed invitation record requires four independent persisted fields"
    )]
    pub const fn new(
        invitation_id: [u8; 16],
        space_id: SpaceId,
        token_hash: [u8; 32],
        expires_at_ms: i64,
    ) -> Self {
        Self {
            invitation_id,
            space_id,
            token_hash,
            expires_at_ms,
        }
    }
}

/// Inputs required to atomically redeem one invitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Redemption {
    pub(crate) token_hash: [u8; 32],
    pub(crate) endpoint_id: EndpointId,
    pub(crate) now_ms: i64,
    pub(crate) consumed_at_ms: i64,
}

impl Redemption {
    /// Creates one redemption attempt.
    #[expect(
        clippy::too_many_arguments,
        reason = "redemption atomically binds token, endpoint, observation, and consumption times"
    )]
    pub const fn new(
        token_hash: [u8; 32],
        endpoint_id: EndpointId,
        now_ms: i64,
        consumed_at_ms: i64,
    ) -> Self {
        Self {
            token_hash,
            endpoint_id,
            now_ms,
            consumed_at_ms,
        }
    }
}

/// Typed result of an atomic invitation redemption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every closed schema-v1 redemption state"
)]
pub enum RedemptionOutcome {
    /// This attempt consumed the pending invitation.
    Redeemed {
        /// Revision committed with the redemption.
        revision: u64,
    },
    /// Another transaction already consumed it.
    AlreadyConsumed,
    /// No invitation or consumed-token record matches the hash.
    NotFound,
    /// The pending invitation is expired.
    Expired,
    /// The invitation was explicitly revoked.
    Revoked,
}

/// One contiguous signed-manifest advancement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestAdvance {
    pub(crate) space_id: SpaceId,
    pub(crate) generation: u64,
    pub(crate) previous_hash: Option<[u8; 32]>,
    pub(crate) manifest_hash: [u8; 32],
    pub(crate) signed_manifest: Vec<u8>,
}

impl ManifestAdvance {
    /// Creates one proposed manifest advancement.
    #[expect(
        clippy::too_many_arguments,
        reason = "the signed manifest chain record has five required persisted fields"
    )]
    pub const fn new(
        space_id: SpaceId,
        generation: u64,
        previous_hash: Option<[u8; 32]>,
        manifest_hash: [u8; 32],
        signed_manifest: Vec<u8>,
    ) -> Self {
        Self {
            space_id,
            generation,
            previous_hash,
            manifest_hash,
            signed_manifest,
        }
    }
}

/// Typed result of signed-manifest advancement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle both closed schema-v1 manifest outcomes"
)]
pub enum ManifestOutcome {
    /// The next contiguous generation was committed.
    Advanced {
        /// Revision committed with the manifest.
        revision: u64,
    },
    /// The generation or previous hash did not extend current state.
    Conflict {
        /// Latest accepted generation, or none before genesis.
        current_generation: Option<u64>,
    },
}

/// Current highest signed address state for one Space member.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct AddressAdvance {
    /// Space that scopes forwarding and authorization.
    pub space_id: SpaceId,
    /// Endpoint whose current reachability is signed.
    pub endpoint_id: EndpointId,
    /// Monotonic Endpoint-owned sequence.
    pub sequence: u64,
    /// Signed record issue time.
    pub issued_at_ms: i64,
    /// Signed record expiry time.
    pub expires_at_ms: i64,
    /// Hash of the signed canonical record.
    pub record_hash: [u8; 32],
    /// Exact signed record bytes retained for forwarding.
    pub signed_record: Vec<u8>,
}

/// Current highest private-relay advertisement for one Space relay Endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct RelayAdvertisementAdvance {
    /// Space in which this private relay is advertised.
    pub space_id: SpaceId,
    /// Hosting Endpoint identity.
    pub relay_endpoint_id: EndpointId,
    /// Monotonic advertisement sequence.
    pub sequence: u64,
    /// Advertisement expiry time.
    pub expires_at_ms: i64,
    /// Hash of the signed canonical advertisement.
    pub advertisement_hash: [u8; 32],
    /// Exact signed advertisement bytes retained for forwarding.
    pub signed_advertisement: Vec<u8>,
}

/// Typed result of highest-sequence state advancement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle both closed schema-v1 sequence outcomes"
)]
pub enum SequenceOutcome {
    /// The higher sequence was committed.
    Advanced {
        /// Revision committed with the higher sequence.
        revision: u64,
    },
    /// The supplied sequence was not higher than current state.
    Stale {
        /// Highest sequence already accepted.
        current_sequence: u64,
    },
}

/// A revocable session represented only by its bearer-token hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct SessionRecord {
    /// Non-recoverable hash of the bearer token.
    pub session_id_hash: [u8; 32],
    /// Session creation time.
    pub created_at_ms: i64,
    /// Absolute session expiry time.
    pub expires_at_ms: i64,
}

/// Password verifier replacement and session-revocation timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "this schema-v1 transaction DTO is intentionally constructible as a complete record"
)]
pub struct PasswordReset {
    /// Argon2id verifier bytes, never a recoverable passphrase.
    pub verifier: Vec<u8>,
    /// Verifier format version.
    pub verifier_version: u32,
    /// Reset and revocation timestamp.
    pub now_ms: i64,
}

/// Effective per-connection `SQLite` safety settings.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "the complete schema-v1 safety snapshot is returned for direct inspection"
)]
pub struct DatabaseSettings {
    /// Persistent journal mode.
    pub journal_mode: String,
    /// Whether foreign-key enforcement is active on this connection.
    pub foreign_keys: bool,
    /// Busy timeout in milliseconds.
    pub busy_timeout_ms: u64,
}
