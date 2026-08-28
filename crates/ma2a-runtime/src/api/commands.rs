//! Closed local Runtime command set.

use ma2a_core::{EndpointId, RequestId, SpaceId};

use super::{ApiError, MAX_TEXT_BYTES};

/// Exact ordered operation inventory carried by the schema and TypeScript contract.
pub const COMMAND_NAMES: [&str; 21] = [
    "handshake",
    "status",
    "endpoint_info",
    "space_create",
    "space_list",
    "space_show",
    "space_invite",
    "space_redeem",
    "space_revoke",
    "control_sync_status",
    "control_sync_trigger",
    "private_relay_configure",
    "private_relay_status",
    "public_relay_configure",
    "public_relay_status",
    "echo_call",
    "ui_password_set",
    "ui_password_reset",
    "session_revoke_all",
    "snapshot_fetch",
    "graceful_shutdown",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundedText(String);

impl BoundedText {
    pub(crate) fn parse(value: &str, maximum: usize) -> Result<Self, ApiError> {
        if value.is_empty() || value.len() > maximum || value.len() > MAX_TEXT_BYTES {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self(value.to_owned()))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateRelayMode {
    NativeTls,
    ExternalTermination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Handshake,
    Status,
    EndpointInfo,
    SpaceCreate(RequestId, BoundedText),
    SpaceList,
    SpaceShow(SpaceId),
    SpaceInvite(RequestId, SpaceId, EndpointId),
    SpaceRedeem(RequestId, BoundedText),
    SpaceRevoke(RequestId, SpaceId, EndpointId),
    ControlSyncStatus(EndpointId),
    ControlSyncTrigger(RequestId, EndpointId),
    PrivateRelayConfigure(RequestId, PrivateRelayMode, BoundedText, u16),
    PrivateRelayStatus,
    PublicRelayConfigure(RequestId, BoundedText),
    PublicRelayStatus,
    EchoCall(RequestId, EndpointId, BoundedText),
    UiPasswordSet(RequestId, BoundedText),
    UiPasswordReset(RequestId, BoundedText),
    SessionRevokeAll(RequestId),
    SnapshotFetch,
    GracefulShutdown(RequestId),
}

/// One parsed member of the exact local API v1 command set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub(crate) kind: CommandKind,
}

impl Command {
    /// Returns the stable operation discriminant.
    pub const fn operation(&self) -> &'static str {
        match self.kind {
            CommandKind::Handshake => "handshake",
            CommandKind::Status => "status",
            CommandKind::EndpointInfo => "endpoint_info",
            CommandKind::SpaceCreate(_, _) => "space_create",
            CommandKind::SpaceList => "space_list",
            CommandKind::SpaceShow(_) => "space_show",
            CommandKind::SpaceInvite(_, _, _) => "space_invite",
            CommandKind::SpaceRedeem(_, _) => "space_redeem",
            CommandKind::SpaceRevoke(_, _, _) => "space_revoke",
            CommandKind::ControlSyncStatus(_) => "control_sync_status",
            CommandKind::ControlSyncTrigger(_, _) => "control_sync_trigger",
            CommandKind::PrivateRelayConfigure(_, _, _, _) => "private_relay_configure",
            CommandKind::PrivateRelayStatus => "private_relay_status",
            CommandKind::PublicRelayConfigure(_, _) => "public_relay_configure",
            CommandKind::PublicRelayStatus => "public_relay_status",
            CommandKind::EchoCall(_, _, _) => "echo_call",
            CommandKind::UiPasswordSet(_, _) => "ui_password_set",
            CommandKind::UiPasswordReset(_, _) => "ui_password_reset",
            CommandKind::SessionRevokeAll(_) => "session_revoke_all",
            CommandKind::SnapshotFetch => "snapshot_fetch",
            CommandKind::GracefulShutdown(_) => "graceful_shutdown",
        }
    }

    /// Returns the mutation correlation identifier, when required.
    pub const fn request_id(&self) -> Option<RequestId> {
        match self.kind {
            CommandKind::Handshake
            | CommandKind::Status
            | CommandKind::EndpointInfo
            | CommandKind::SpaceList
            | CommandKind::SpaceShow(_)
            | CommandKind::ControlSyncStatus(_)
            | CommandKind::PrivateRelayStatus
            | CommandKind::PublicRelayStatus
            | CommandKind::SnapshotFetch => None,
            CommandKind::SpaceCreate(id, _)
            | CommandKind::SpaceInvite(id, _, _)
            | CommandKind::SpaceRedeem(id, _)
            | CommandKind::SpaceRevoke(id, _, _)
            | CommandKind::ControlSyncTrigger(id, _)
            | CommandKind::PrivateRelayConfigure(id, _, _, _)
            | CommandKind::PublicRelayConfigure(id, _)
            | CommandKind::EchoCall(id, _, _)
            | CommandKind::UiPasswordSet(id, _)
            | CommandKind::UiPasswordReset(id, _)
            | CommandKind::SessionRevokeAll(id)
            | CommandKind::GracefulShutdown(id) => Some(id),
        }
    }
}
