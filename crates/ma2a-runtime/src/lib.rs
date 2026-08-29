//! Runtime orchestration boundary for MA2A.

#![deny(unsafe_code)]

/// Transport-neutral local Runtime API v1.
pub mod api;
/// Current-user private local IPC transport.
pub mod ipc;

mod actor;
mod clock;
mod enrollment;
mod enrollment_actor;
mod error;
mod lifecycle;
mod state;
mod store;
mod store_client;

pub use actor::RuntimeHandle;
pub use clock::RuntimeClock;
pub use enrollment::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EnrollmentErrorCode,
    EstablishedEnrollment,
};
pub use error::{RuntimeError, RuntimeErrorCode};
pub use lifecycle::Runtime;
pub use state::{Connectivity, RuntimeEvent, RuntimeStatus, ShutdownReport};
