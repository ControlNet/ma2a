//! Closed local Runtime command set.

use ma2a_core::{EndpointId, RequestId, SpaceId};
use zeroize::Zeroizing;

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

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BoundedText(Zeroizing<String>);

impl BoundedText {
    pub(crate) fn parse(value: &str, maximum: usize) -> Result<Self, ApiError> {
        if value.is_empty() || value.len() > maximum || value.len() > MAX_TEXT_BYTES {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self(Zeroizing::new(value.to_owned())))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn parse_secret(value: Zeroizing<String>, maximum: usize) -> Result<Self, ApiError> {
        if value.is_empty() || value.len() > maximum || value.len() > MAX_TEXT_BYTES {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self(value))
        }
    }

    fn into_secret(self) -> Zeroizing<String> {
        self.0
    }
}

pub(crate) enum UiControlCommand {
    PasswordSet(Zeroizing<String>),
    PasswordReset(Zeroizing<String>),
    SessionsRevokeAll,
}

impl std::fmt::Debug for BoundedText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BoundedText([REDACTED])")
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
    pub(crate) fn into_ui_control(self) -> Result<UiControlCommand, ApiError> {
        match self.kind {
            CommandKind::UiPasswordSet(_, password) => {
                Ok(UiControlCommand::PasswordSet(password.into_secret()))
            }
            CommandKind::UiPasswordReset(_, password) => {
                Ok(UiControlCommand::PasswordReset(password.into_secret()))
            }
            CommandKind::SessionRevokeAll(_) => Ok(UiControlCommand::SessionsRevokeAll),
            CommandKind::Handshake
            | CommandKind::Status
            | CommandKind::EndpointInfo
            | CommandKind::SpaceCreate(_, _)
            | CommandKind::SpaceList
            | CommandKind::SpaceShow(_)
            | CommandKind::SpaceInvite(_, _, _)
            | CommandKind::SpaceRedeem(_, _)
            | CommandKind::SpaceRevoke(_, _, _)
            | CommandKind::ControlSyncStatus(_)
            | CommandKind::ControlSyncTrigger(_, _)
            | CommandKind::PrivateRelayConfigure(_, _, _, _)
            | CommandKind::PrivateRelayStatus
            | CommandKind::PublicRelayConfigure(_, _)
            | CommandKind::PublicRelayStatus
            | CommandKind::EchoCall(_, _, _)
            | CommandKind::SnapshotFetch
            | CommandKind::GracefulShutdown(_) => Err(ApiError::invalid_input()),
        }
    }

    /// Creates a typed Todo 4 UI password-set command with a fresh request identifier.
    ///
    /// # Errors
    /// Returns a typed local API failure when randomness or password bounds fail.
    pub fn ui_password_set(password: Zeroizing<String>) -> Result<Self, ApiError> {
        Ok(Self {
            kind: CommandKind::UiPasswordSet(
                RequestId::random().map_err(ApiError::new)?,
                BoundedText::parse_secret(password, 1_024)?,
            ),
        })
    }

    /// Creates a typed Todo 4 UI password-reset command with a fresh request identifier.
    ///
    /// # Errors
    /// Returns a typed local API failure when randomness or password bounds fail.
    pub fn ui_password_reset(password: Zeroizing<String>) -> Result<Self, ApiError> {
        Ok(Self {
            kind: CommandKind::UiPasswordReset(
                RequestId::random().map_err(ApiError::new)?,
                BoundedText::parse_secret(password, 1_024)?,
            ),
        })
    }

    /// Creates a typed Todo 4 session revoke-all command with a fresh request identifier.
    ///
    /// # Errors
    /// Returns a typed local API failure when request identifier generation fails.
    pub fn session_revoke_all() -> Result<Self, ApiError> {
        Ok(Self {
            kind: CommandKind::SessionRevokeAll(RequestId::random().map_err(ApiError::new)?),
        })
    }

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

    pub(crate) const fn control_sync_peer(&self) -> Option<EndpointId> {
        match self.kind {
            CommandKind::ControlSyncStatus(peer) | CommandKind::ControlSyncTrigger(_, peer) => {
                Some(peer)
            }
            CommandKind::Handshake
            | CommandKind::Status
            | CommandKind::EndpointInfo
            | CommandKind::SpaceCreate(_, _)
            | CommandKind::SpaceList
            | CommandKind::SpaceShow(_)
            | CommandKind::SpaceInvite(_, _, _)
            | CommandKind::SpaceRedeem(_, _)
            | CommandKind::SpaceRevoke(_, _, _)
            | CommandKind::PrivateRelayConfigure(_, _, _, _)
            | CommandKind::PrivateRelayStatus
            | CommandKind::PublicRelayConfigure(_, _)
            | CommandKind::PublicRelayStatus
            | CommandKind::EchoCall(_, _, _)
            | CommandKind::UiPasswordSet(_, _)
            | CommandKind::UiPasswordReset(_, _)
            | CommandKind::SessionRevokeAll(_)
            | CommandKind::SnapshotFetch
            | CommandKind::GracefulShutdown(_) => None,
        }
    }

    pub(crate) fn echo_call(&self) -> Option<(RequestId, EndpointId, &str)> {
        match &self.kind {
            CommandKind::EchoCall(request_id, target, payload) => {
                Some((*request_id, *target, payload.as_str()))
            }
            CommandKind::Handshake
            | CommandKind::Status
            | CommandKind::EndpointInfo
            | CommandKind::SpaceCreate(_, _)
            | CommandKind::SpaceList
            | CommandKind::SpaceShow(_)
            | CommandKind::SpaceInvite(_, _, _)
            | CommandKind::SpaceRedeem(_, _)
            | CommandKind::SpaceRevoke(_, _, _)
            | CommandKind::ControlSyncStatus(_)
            | CommandKind::ControlSyncTrigger(_, _)
            | CommandKind::PrivateRelayConfigure(_, _, _, _)
            | CommandKind::PrivateRelayStatus
            | CommandKind::PublicRelayConfigure(_, _)
            | CommandKind::PublicRelayStatus
            | CommandKind::UiPasswordSet(_, _)
            | CommandKind::UiPasswordReset(_, _)
            | CommandKind::SessionRevokeAll(_)
            | CommandKind::SnapshotFetch
            | CommandKind::GracefulShutdown(_) => None,
        }
    }
}
