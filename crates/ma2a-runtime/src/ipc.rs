//! Current-user, exact-version local Runtime transport.

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

mod client;
mod framing;
mod lifecycle;
mod server;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(any(windows, test))]
mod windows_security;

pub use client::LocalApiClient;
pub use lifecycle::{
    DaemonIdentity, DaemonReport, LifecycleRequest, ProcessIncarnation, RuntimeBoot,
    random_launch_nonce,
};
pub use server::{LocalApiServer, ServerExit};

/// Bounds the transfer of one frame's bytes once the frame has begun arriving.
///
/// This is a transport property: a peer that has started a frame and then stalls
/// is malformed, whatever the command behind it was doing. It is deliberately the
/// only deadline this transport imposes. Waiting for a local Runtime to finish a
/// command needs no deadline, because a Runtime that stops is a Runtime whose
/// socket closes, and the read then ends immediately and exactly. Guessing how
/// long legitimate work may take instead produced failures for work that was
/// still progressing, which is what this transport must never report.
const IO_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Bounds one complete lifecycle call, unlike a business call which is unbounded.
///
/// A lifecycle answer is assembled from data the daemon already holds, so the
/// only thing that can make one slow is a daemon that is no longer able to serve
/// — which is precisely the answer the caller is trying to obtain. Waiting past
/// this point cannot turn into a reply, so the wait is converted into the
/// `Unresponsive` verdict instead of hanging the command that asked.
pub const LIFECYCLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// In-flight connections available to ordinary local API traffic.
///
/// This is the original concurrency budget and is deliberately unchanged by the
/// lifecycle reserve below: business traffic still gets all 32 of it.
const BUSINESS_CONNECTION_LIMIT: usize = 32;

/// Connection slots kept back so lifecycle calls survive a saturated daemon.
///
/// Business connections that are all blocked on the same stuck Runtime would
/// otherwise consume every slot, and a caller asking "are you alive?" would be
/// dropped without an answer by the daemon it is asking about. Connections taken
/// from this reserve answer lifecycle calls and refuse everything else.
///
/// These are additional to [`BUSINESS_CONNECTION_LIMIT`] rather than carved out
/// of it, so the daemon accepts at most 36 connections in total. Carving them
/// out instead would have quietly reduced the business budget to 28.
const LIFECYCLE_RESERVE: usize = 4;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Filesystem locations reserved for one user's daemon instance.
pub struct IpcPaths {
    state_dir: PathBuf,
    runtime_dir: PathBuf,
}

impl IpcPaths {
    /// Derives private transport paths from an absolute state directory.
    ///
    /// # Errors
    /// Returns [`IpcError::InvalidPath`] when `state_dir` is relative.
    pub fn new(state_dir: impl AsRef<Path>) -> Result<Self, IpcError> {
        let state_dir = state_dir.as_ref();
        if !state_dir.is_absolute() {
            return Err(IpcError::InvalidPath);
        }
        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            runtime_dir: state_dir.join("run-v1"),
        })
    }

    /// Returns the persistent state directory.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Returns the owner-private runtime directory.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Returns the singleton lock file path.
    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.lock")
    }

    /// Returns the lock that serializes on-demand daemon startup.
    pub fn startup_lock_path(&self) -> PathBuf {
        self.runtime_dir.join("startup.lock")
    }

    /// Returns the owner-private file a detached daemon writes diagnostics to.
    pub fn log_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.log")
    }

    /// Returns the owner-private record describing the daemon launch in charge.
    ///
    /// This file is a witness, never an authority. [`Self::lock_path`] decides who
    /// owns the state directory; this only says which launch, process incarnation
    /// and Runtime boot that owner claims to be, so a caller can tell a stale
    /// record from the daemon actually answering.
    pub fn daemon_record_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.json")
    }

    #[cfg(unix)]
    /// Returns the Unix domain socket path.
    pub fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join("control.sock")
    }

    /// Creates and validates the platform transport directory.
    ///
    /// # Errors
    /// Returns an I/O or authorization error when the directory is not owner-private.
    pub fn prepare(&self) -> Result<(), IpcError> {
        platform::prepare(self)
    }

    /// Removes an endpoint left by a daemon that no longer owns the startup lock.
    ///
    /// # Errors
    /// Returns an I/O error when the stale endpoint cannot be removed.
    pub fn remove_stale_endpoint(&self) -> Result<(), IpcError> {
        platform::remove_stale_endpoint(self)
    }
}

#[derive(Debug)]
#[non_exhaustive]
/// Failure returned by the private local transport.
pub enum IpcError {
    /// A transport path was not absolute or could not become a platform name.
    InvalidPath,
    /// The runtime directory or connected peer is not owned by the current user.
    UnauthorizedPeer,
    /// A frame was empty, oversized, incomplete, or stalled mid-transfer.
    InvalidFrame,
    /// A response did not carry the request correlation identifier.
    CorrelationMismatch,
    /// The daemon handshake did not report the exact supported API version.
    VersionMismatch,
    /// Something answered the endpoint but does not speak the lifecycle protocol.
    ///
    /// This is a fact about the responder, never about whether a daemon exists.
    /// A caller that sees it falls back to the versioned handshake to find out
    /// what it is talking to.
    NoLifecyclePlane,
    /// Platform transport I/O failed.
    Io(io::Error),
    /// Local API parsing or encoding failed.
    Api(api::ApiError),
    /// Runtime command execution failed.
    Runtime(crate::RuntimeError),
    /// A structured connection task failed to join.
    Join(tokio::task::JoinError),
}

impl fmt::Display for IpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => formatter.write_str("private IPC path is not absolute"),
            Self::UnauthorizedPeer => {
                formatter.write_str("private IPC peer is not the current user")
            }
            Self::InvalidFrame => formatter.write_str("private IPC frame is invalid"),
            Self::CorrelationMismatch => {
                formatter.write_str("private IPC response correlation mismatch")
            }
            Self::VersionMismatch => formatter.write_str("private IPC handshake version mismatch"),
            Self::NoLifecyclePlane => {
                formatter.write_str("the daemon does not speak the lifecycle protocol")
            }
            Self::Io(error) => write!(formatter, "private IPC I/O failed: {error}"),
            Self::Api(error) => write!(formatter, "local API failed: {error}"),
            Self::Runtime(error) => write!(formatter, "Runtime request failed: {error}"),
            Self::Join(error) => write!(formatter, "private IPC task failed: {error}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<io::Error> for IpcError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<api::ApiError> for IpcError {
    fn from(error: api::ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<crate::RuntimeError> for IpcError {
    fn from(error: crate::RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<tokio::task::JoinError> for IpcError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::Join(error)
    }
}

use crate::api;
