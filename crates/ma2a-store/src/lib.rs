//! Persistent-state adapter boundary for MA2A.
//!
//! Raw relay advertisement persistence is intentionally not public:
//! ```compile_fail
//! use ma2a_store::RelayAdvertisementAdvance;
//! ```

#![forbid(unsafe_code)]

mod address_record_queries;
mod address_records;
mod backup;
mod committed;
mod config;
mod control_batch;
mod control_state;
mod enrollment;
mod enrollment_models;
mod error;
mod key_store;
mod migrations;
mod mutation_replay;
mod owned_spaces;
mod permissions;
mod relay_activity;
mod relay_advertisement;
mod relay_config;
mod relay_settings;
mod relay_state;
mod repository;
mod repository_models;
mod repository_mutations;
mod repository_state;
mod repository_state_models;
mod revision;
mod session_mutations;
mod sessions;
mod space_rows;
mod spaces;
mod web_auth;

pub use address_record_queries::PersistedAddressRecord;
pub use address_records::{
    AddressRecordBoundaryError, AddressRecordOutcome, AddressRecordTarget, AddressRecordValidation,
    ValidatedAddressRecord,
};
pub use committed::Committed;
pub use config::StoreConfig;
pub use control_batch::ControlBatch;
pub use control_state::ControlSpaceState;
pub use enrollment::CreatedEnrollmentInvite;
pub use enrollment_models::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, EnrollmentRedemption,
};
pub use error::StoreError;
pub use key_store::{KeyKind, KeyMaterial, KeyReference, KeyStore, ProtectedSecret};
pub use migrations::SCHEMA_VERSION;
pub use mutation_replay::{
    LOCAL_MUTATION_REPLAY_MAX_BYTES, LOCAL_MUTATION_REPLAY_MAX_ENTRIES,
    LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES, MutationReplayRecord, MutationReplayRequest,
    MutationReplayState,
};
pub use owned_spaces::{AdvancedOwnedSpace, CreatedSpace, OwnedSpaceUpdate, SpaceCreation};
pub use relay_advertisement::{RelayAdvertisementBoundaryError, ValidatedRelayAdvertisement};
pub use relay_config::{PersistedRelayAdvertisement, RelayAdvertisementOutcome};
pub use repository::Repository;
pub use repository_models::{
    DatabaseSettings, EndpointRecord, InvitationRecord, ManifestAdvance, ManifestOutcome,
    PasswordReset, Redemption, RedemptionOutcome, SpaceRecord,
};
pub use repository_state_models::{
    EndpointObservationUpdate, RelayConfiguration, RelayObservation, RelayTransportConfiguration,
    RuntimeMetadata, RuntimeMetadataUpdate,
};
pub use revision::{SnapshotMember, SnapshotSpace, SnapshotState, SpaceDetails};
pub use sessions::{
    SessionAdmission, SessionCreate, SessionDigests, SessionRecord, SessionTimestamps, SessionTouch,
};
pub use spaces::SpaceChainPersistence;
pub use web_auth::{
    ARGON2_MEMORY_KIB, ARGON2_OUTPUT_BYTES, ARGON2_PARALLELISM, ARGON2_TIME_COST, CredentialRecord,
    PasswordTransition, derive_password_verifier, verify_password,
};
