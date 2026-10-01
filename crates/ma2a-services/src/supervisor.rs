//! Per-Process supervisors: the owner of one durable workload, its output, and its status.
//!
//! A supervisor is a detached process that outlives the daemon that launched it. The daemon
//! re-adopts it after a restart only through the mutual ownership proof in [`proof`]; a
//! process identifier is never evidence that a workload is running.

mod address;
mod host;
mod host_attach;
mod host_child;
mod host_serve;
pub mod launch;
pub mod log;
pub mod proof;
pub mod protocol;
pub mod status;

pub use address::{CONNECT_DEADLINE, Channel, SupervisorAddress};
pub use host::{detach_session, log_directory, supervise};
pub use host_child::GO_DEADLINE;
pub use launch::Launch;
pub use proof::Credentials;
pub use protocol::SupervisorRequest;
pub use status::{EndCause, SupervisorStatus};

/// Ends this process at once, as a crash would, for the deterministic fault points of the
/// recovery tests (debug builds only).
///
/// `SIGKILL` rather than `abort`: an abort runs the system's core-dump handler, which can
/// hold the dying process for many seconds and so hide the very timing a test controls.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn crash() -> ! {
    #[cfg(unix)]
    {
        let _killed =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
    }
    std::process::exit(134)
}
