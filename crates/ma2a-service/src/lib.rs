//! Rust SDK for services hosted through a ma2a Runtime, and for calling them.
//!
//! A hosted service is an ordinary local program. It never touches the network and never sees
//! a Space: the Runtime it attaches to authenticates every caller, authorizes every call, and
//! hands over only calls it has admitted.
//!
//! - [`ServiceHost`] serves one registered service: one handler per `(version, operation)`,
//!   concurrent calls, and reconnection after the link or the Runtime is lost.
//! - [`Call`] is one admitted call: who called, under which `RequestId`, which version and
//!   operation, the body, and the Services v1 frames to answer it.
//! - [`Caller`] discovers and calls services on another Endpoint through this user's Runtime.
//!
//! The link protocol is language-neutral (`docs/protocol/service-v1.md`); this crate is one
//! implementation of it.

mod call;
mod caller;
mod host;
mod wire;

use std::fmt;

use ma2a_runtime::ipc::IpcError;

pub use call::{Call, CallInput, CallOutput, Ended, Input};
pub use caller::{CallStream, Caller, Received};
pub use host::ServiceHost;
pub use ma2a_core::service::{
    Channel, DescribePage, HostedCall, Interaction, OperationName, ServiceDescriptor, ServiceError,
    ServiceManifest, ServiceName, ServiceVersion, select_version,
};

/// Why an SDK operation failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The local Runtime could not be reached, or the connection broke.
    Ipc(IpcError),
    /// The local Runtime refused the request with this local API error.
    Refused {
        /// The local API error name, such as `not_found` or `unavailable`.
        error: String,
        /// What the operator can do about it, when the Runtime says.
        remediation: Option<String>,
    },
    /// The service, or the remote Endpoint, answered with this service error.
    Service(ServiceError),
    /// The registered manifest lists an operation this host has no handler for.
    MissingHandler {
        /// The version listing the operation.
        version: u16,
        /// The operation without a handler.
        operation: String,
    },
    /// A frame or message broke the protocol, or an argument exceeds its bound.
    Protocol(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ipc(error) => write!(formatter, "local Runtime: {error}"),
            Self::Refused {
                error,
                remediation: Some(remediation),
            } => write!(formatter, "the Runtime refused ({error}): {remediation}"),
            Self::Refused { error, .. } => write!(formatter, "the Runtime refused ({error})"),
            Self::Service(error) => write!(formatter, "{error} ({})", error.name()),
            Self::MissingHandler { version, operation } => write!(
                formatter,
                "version {version} lists operation {operation}, which has no handler"
            ),
            Self::Protocol(detail) => write!(formatter, "protocol: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Whether an attach that failed with this error may succeed later without anyone acting.
    ///
    /// A Runtime that is down, restarting, or busy, a service not yet started, and a link
    /// another host still holds are retried. A Runtime of another local API version, an IPC
    /// path that is not this user's, an unregistered service, any other refusal, and a
    /// manifest this host cannot serve stay that way, so they are returned instead.
    fn retries(&self) -> bool {
        match self {
            Self::Ipc(
                IpcError::VersionMismatch | IpcError::UnauthorizedPeer | IpcError::InvalidPath,
            )
            | Self::Protocol(_)
            | Self::MissingHandler { .. } => false,
            Self::Refused { error, .. } => {
                matches!(error.as_str(), "unavailable" | "conflict" | "internal")
            }
            Self::Ipc(_) | Self::Service(_) => true,
        }
    }
}

impl From<IpcError> for Error {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<ServiceError> for Error {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}
