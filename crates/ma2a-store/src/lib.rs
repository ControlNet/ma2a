//! Persistent-state adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod address_records;
mod backup;
mod config;
mod enrollment;
mod enrollment_models;
mod error;
mod key_store;
mod migrations;
mod owned_spaces;
mod permissions;
mod relay_config;
mod relay_settings;
mod repository;
mod repository_models;
mod repository_mutations;
mod repository_sequences;
mod repository_state;
mod repository_state_models;
mod session_mutations;
mod sessions;
mod space_rows;
mod spaces;
mod web_auth;

pub use address_records::{AddressRecordOutcome, PersistedAddressRecord};
pub use config::StoreConfig;
pub use enrollment_models::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, EnrollmentRedemption,
};
pub use error::StoreError;
pub use key_store::{KeyKind, KeyMaterial, KeyReference, KeyStore, ProtectedSecret};
pub use migrations::SCHEMA_VERSION;
pub use owned_spaces::{AdvancedOwnedSpace, CreatedSpace, OwnedSpaceUpdate, SpaceCreation};
pub use relay_config::{PersistedRelayAdvertisement, RelayAdvertisementOutcome};
pub use repository::Repository;
pub use repository_models::{
    AddressAdvance, DatabaseSettings, EndpointRecord, InvitationRecord, ManifestAdvance,
    ManifestOutcome, PasswordReset, Redemption, RedemptionOutcome, RelayAdvertisementAdvance,
    SequenceOutcome, SpaceRecord,
};
pub use repository_state_models::{
    EndpointObservationUpdate, RelayConfiguration, RelayObservation, RelayTransportConfiguration,
    RuntimeMetadata, RuntimeMetadataUpdate,
};
pub use sessions::{
    SessionAdmission, SessionCreate, SessionDigests, SessionRecord, SessionTimestamps, SessionTouch,
};
pub use spaces::SpaceChainPersistence;
pub use web_auth::{
    ARGON2_MEMORY_KIB, ARGON2_OUTPUT_BYTES, ARGON2_PARALLELISM, ARGON2_TIME_COST, CredentialRecord,
    PasswordTransition, derive_password_verifier, verify_password,
};
