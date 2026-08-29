//! Persistent-state adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod backup;
mod config;
mod enrollment;
mod enrollment_models;
mod error;
mod key_store;
mod migrations;
mod owned_spaces;
mod permissions;
mod repository;
mod repository_models;
mod repository_mutations;
mod repository_sequences;
mod repository_state;
mod repository_state_models;
mod space_rows;
mod spaces;

pub use config::StoreConfig;
pub use enrollment_models::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, EnrollmentRedemption,
};
pub use error::StoreError;
pub use key_store::{KeyKind, KeyMaterial, KeyReference, KeyStore, ProtectedSecret};
pub use migrations::SCHEMA_VERSION;
pub use owned_spaces::{AdvancedOwnedSpace, CreatedSpace, OwnedSpaceUpdate, SpaceCreation};
pub use repository::Repository;
pub use repository_models::{
    AddressAdvance, DatabaseSettings, EndpointRecord, InvitationRecord, ManifestAdvance,
    ManifestOutcome, PasswordReset, Redemption, RedemptionOutcome, RelayAdvertisementAdvance,
    SequenceOutcome, SessionRecord, SpaceRecord,
};
pub use repository_state_models::{
    EndpointObservationUpdate, RelayConfiguration, RelayObservation, RuntimeMetadata,
    RuntimeMetadataUpdate,
};
pub use spaces::SpaceChainPersistence;
