//! Persistent-state adapter boundary for MA2A.
//!
//! Raw relay advertisement persistence is intentionally not public:
//! ```compile_fail
//! use ma2a_store::RelayAdvertisementAdvance;
//! ```

#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

mod address_record_queries;
mod address_records;
mod agent_actions;
mod agent_log;
mod agent_output;
mod agent_reads;
mod agent_rows;
mod agent_tasks;
mod agent_tools;
mod agents;
mod backup;
mod committed;
mod config;
mod control_batch;
mod control_state;
mod enrollment;
mod enrollment_projection;
mod hosted_services;
pub use enrollment_projection::EnrollmentProjection;
mod departure_receipts;
mod enrollment_models;
mod error;
mod key_store;
mod migrations;
mod mutation_replay;
mod owned_spaces;
mod permissions;
mod process_retention;
mod process_rows;
mod processes;
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
mod service_exposure_rows;
mod service_exposures;
mod session_mutations;
mod sessions;
mod space_capacity;
mod space_rows;
mod spaces;
mod transfer_staging;
mod web_auth;

pub use address_record_queries::PersistedAddressRecord;
pub use address_records::{
    AddressRecordBoundaryError, AddressRecordOutcome, AddressRecordTarget, AddressRecordValidation,
    ValidatedAddressRecord,
};
pub use agent_actions::{AgentTaskChange, ToolAnswer};
pub use agent_log::{
    AgentDelivery, MAX_DURABLE_EVENTS, MAX_PROGRESS_EVENTS, MAX_UNACKED_DELIVERIES,
};
pub use agent_output::{AgentOutput, AgentOutputApplied, AgentOutputEffect};
pub use agent_reads::{AGENT_TASK_RETENTION_MS, AgentOutboxEntry, AgentTaskFilter};
pub use agent_rows::{AgentDefinition, AgentInstanceRecord, AgentRecord, AgentTaskRecord};
pub use agent_tasks::{
    AgentTaskCreation, MAX_LIVE_TASKS, MAX_LIVE_TASKS_PER_CREATOR, MAX_RETAINED_TASKS,
    MAX_RETAINED_TASKS_PER_CREATOR, NewAgentTask,
};
pub use agent_tools::{AgentToolCall, MAX_OPEN_TOOL_CALLS_PER_TASK, MAX_TOOL_CALLS_PER_TASK};
pub use agents::AgentDefine;
pub use committed::Committed;
pub use config::StoreConfig;
pub use control_batch::ControlBatch;
pub use control_state::ControlSpaceState;
pub use departure_receipts::{DepartureRequest, MAX_BOUND_DEPARTURES};
pub use enrollment::CreatedEnrollmentInvite;
pub use enrollment_models::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, EnrollmentRedemption,
};
pub use error::StoreError;
pub use hosted_services::{HostedServiceRecord, ServiceRegistration};
pub use key_store::{KeyKind, KeyMaterial, KeyReference, KeyStore, ProtectedSecret};
pub use migrations::SCHEMA_VERSION;
pub use mutation_replay::{
    LOCAL_MUTATION_REPLAY_MAX_BYTES, LOCAL_MUTATION_REPLAY_MAX_ENTRIES,
    LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES, MutationReplayRecord, MutationReplayRequest,
    MutationReplayState,
};
pub use owned_spaces::{AdvancedOwnedSpace, CreatedSpace, OwnedSpaceUpdate, SpaceCreation};
pub use permissions::{
    create_private_directory, ensure_private_directory, is_local_absolute,
    protect_private_directory, protect_private_file,
};
pub use process_retention::{ENDED_RETENTION_MS, MAX_TOMBSTONES, TOMBSTONE_RETENTION_MS};
pub use process_rows::{ProcessRecord, ProcessTimes};
pub use processes::{
    MAX_ACTIVE_PROCESSES, MAX_ACTIVE_PROCESSES_PER_CREATOR, MAX_RETAINED_PROCESSES,
    MAX_RETAINED_PROCESSES_PER_CREATOR, NewProcess, ProcessArchive, ProcessCreation, ProcessFilter,
    ProcessTransition,
};
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
pub use service_exposures::{ExposureRecord, FileRootExposure, LocalExposure};
pub use sessions::{
    SessionAdmission, SessionCreate, SessionDigests, SessionRecord, SessionTimestamps, SessionTouch,
};
pub use spaces::SpaceChainPersistence;
pub use transfer_staging::TransferStaging;
pub use web_auth::{
    ARGON2_MEMORY_KIB, ARGON2_OUTPUT_BYTES, ARGON2_PARALLELISM, ARGON2_TIME_COST, CredentialRecord,
    PasswordTransition, derive_password_verifier, verify_password,
};
