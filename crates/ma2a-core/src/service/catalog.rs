//! The catalog of built-in families: one entry per family drives its admission, its stream
//! bounds, and what discovery says about it.
//!
//! Hosted services go through the same pipeline; their entry comes from the Store instead.

use crate::{ProtocolError, ServiceKind};

use super::{Interaction, OperationName, ServiceAccess, ServiceDescriptor, ServiceOrigin};

use Interaction::{Bytes, Resource, Stream, Unary};

/// The protocol version every built-in family speaks.
pub const BUILTIN_VERSION: u16 = 1;

/// Returns the operations of one built-in family, by the names the audit uses.
pub const fn builtin_operations(kind: ServiceKind) -> &'static [(&'static str, Interaction)] {
    match kind {
        ServiceKind::Echo => &[("echo", Unary)],
        ServiceKind::Files => &[
            ("stat", Unary),
            ("list", Unary),
            ("mkdir", Unary),
            ("read", Unary),
            ("write", Unary),
            ("rename", Unary),
            ("remove", Unary),
        ],
        ServiceKind::Transfer => &[("download", Bytes), ("upload", Bytes)],
        ServiceKind::Exec => &[("exec", Bytes)],
        ServiceKind::Shell => &[("shell", Bytes)],
        ServiceKind::Proxy => &[("proxy", Bytes)],
        ServiceKind::Process => &[
            ("process_start", Resource),
            ("process_list", Unary),
            ("process_inspect", Unary),
            ("process_logs", Stream),
            ("process_attach", Bytes),
            ("process_stdin", Bytes),
            ("process_signal", Unary),
            ("process_terminate", Unary),
            ("process_delete", Unary),
        ],
        ServiceKind::Agent => &[
            ("agent_discover", Unary),
            ("agent_submit", Resource),
            ("agent_send", Unary),
            ("agent_get", Unary),
            ("agent_tasks", Unary),
            ("agent_watch", Stream),
            ("agent_cancel", Unary),
            ("agent_artifact", Bytes),
        ],
        ServiceKind::Service => &[("service_describe", Unary), ("service_call", Stream)],
    }
}

/// Returns the fixed description of one built-in family.
pub const fn builtin_description(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Echo => "Bounded encrypted echo.",
        ServiceKind::Files => "Bounded filesystem operations on exported roots.",
        ServiceKind::Transfer => "Large-content transfer with integrity verification.",
        ServiceKind::Exec => "One-shot program execution.",
        ServiceKind::Shell => "Interactive terminal sessions.",
        ServiceKind::Proxy => "TCP byte streams to allowed destinations.",
        ServiceKind::Process => "Durable Processes that outlive the operation that started them.",
        ServiceKind::Agent => "Tasks for the Agents this Endpoint hosts.",
        ServiceKind::Service => "Discovery of, and calls to, the services this Endpoint hosts.",
    }
}

/// Returns the coarse admission evaluated before any byte of a stream is read.
pub const fn admission(kind: ServiceKind) -> Option<ServiceAccess> {
    match kind {
        ServiceKind::Echo => None,
        ServiceKind::Files | ServiceKind::Transfer => Some(ServiceAccess::FilesAdmission),
        ServiceKind::Exec => Some(ServiceAccess::Exec),
        ServiceKind::Shell => Some(ServiceAccess::Shell),
        ServiceKind::Proxy => Some(ServiceAccess::ProxyAdmission),
        ServiceKind::Process => Some(ServiceAccess::Process),
        ServiceKind::Agent => Some(ServiceAccess::AgentAdmission),
        ServiceKind::Service => Some(ServiceAccess::Discover),
    }
}

/// Returns the fixed `(per peer, per Runtime)` bounds on concurrent streams of one family.
pub const fn stream_limits(kind: ServiceKind) -> (usize, usize) {
    match kind {
        ServiceKind::Echo => (16, 128),
        ServiceKind::Files | ServiceKind::Process | ServiceKind::Service => (16, 64),
        ServiceKind::Transfer | ServiceKind::Shell => (4, 16),
        ServiceKind::Exec => (8, 32),
        ServiceKind::Proxy => (64, 256),
        // Watches are long-lived, so an Agent caller may hold several at once.
        ServiceKind::Agent => (32, 128),
    }
}

/// Returns what discovery says about one built-in family.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] only if the catalog itself were malformed, which
/// the catalog tests rule out.
pub fn builtin_descriptor(kind: ServiceKind) -> Result<ServiceDescriptor, ProtocolError> {
    let operations = builtin_operations(kind)
        .iter()
        .map(|(name, interaction)| Ok((OperationName::new(name)?, *interaction)))
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    let version = super::ServiceVersion::new(BUILTIN_VERSION, operations)?;
    ServiceDescriptor::new(
        ServiceOrigin::Builtin(kind),
        builtin_description(kind),
        (vec![version], true),
    )
}
