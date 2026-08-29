//! Runtime orchestration boundary for MA2A.

#![forbid(unsafe_code)]

/// Transport-neutral local Runtime API v1.
pub mod api;
/// Current-user private local IPC transport.
pub mod ipc;

mod actor;
mod error;
mod lifecycle;
mod state;
mod store;

pub use actor::RuntimeHandle;
pub use error::{RuntimeError, RuntimeErrorCode};
pub use lifecycle::Runtime;
pub use state::{Connectivity, RuntimeEvent, RuntimeStatus, ShutdownReport};
