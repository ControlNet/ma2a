//! Runtime orchestration boundary for MA2A.

#![deny(unsafe_code)]

/// Transport-neutral local Runtime API v1.
pub mod api;
/// Current-user Runtime composition and typed CLI control.
pub mod current_user;
/// Current-user private local IPC transport.
pub mod ipc;
/// Loopback-only browser authentication and HTTP serving.
pub mod web;

mod actor;
mod authz;
mod clock;
mod control_actor;
mod control_sync;
mod enrollment;
mod enrollment_actor;
mod error;
mod lifecycle;
mod reachability;
mod state;
mod store;
mod store_client;

pub use actor::RuntimeHandle;
pub use authz::authorize_remote;
pub use clock::RuntimeClock;
pub use enrollment::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EnrollmentErrorCode,
    EstablishedEnrollment,
};
pub use error::{RuntimeError, RuntimeErrorCode};
pub use lifecycle::Runtime;
pub use state::{Connectivity, RuntimeEvent, RuntimeStatus, ShutdownReport};
