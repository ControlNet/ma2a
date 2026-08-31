//! Closed local Runtime command set.

use ma2a_core::{EndpointId, RequestId, SpaceId};
use zeroize::Zeroizing;

use super::{ApiError, MAX_TEXT_BYTES};

#[path = "command_accessors.rs"]
mod accessors;

/// Exact ordered operation inventory carried by the schema and TypeScript contract.
pub const COMMAND_NAMES: [&str; 24] = [
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
    "private_relay_disable",
    "private_relay_status",
    "public_relay_configure",
    "public_relay_disable",
    "public_relay_status",
    "echo_call",
    "ui_password_set",
    "ui_password_reset",
    "session_revoke_all",
    "ui_open",
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
pub(crate) struct PrivateRelayConfiguration {
    pub(crate) mode: PrivateRelayMode,
    pub(crate) listen: BoundedText,
    pub(crate) public_url: BoundedText,
    pub(crate) served_spaces: Vec<SpaceId>,
    pub(crate) certificate_path: Option<BoundedText>,
    pub(crate) private_key_path: Option<BoundedText>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Handshake,
    Status,
    EndpointInfo,
    SpaceCreate(RequestId, BoundedText),
    SpaceList,
    SpaceShow(SpaceId),
    SpaceInvite(RequestId, SpaceId, u64, BoundedText),
    SpaceRedeem(RequestId, BoundedText),
    SpaceRevoke(RequestId, SpaceId, EndpointId),
    ControlSyncStatus(EndpointId),
    ControlSyncTrigger(RequestId, EndpointId),
    PrivateRelayConfigure(RequestId, PrivateRelayConfiguration),
    PrivateRelayDisable(RequestId),
    PrivateRelayStatus,
    PublicRelayConfigure(RequestId, BoundedText),
    PublicRelayDisable(RequestId),
    PublicRelayStatus,
    EchoCall(RequestId, EndpointId, BoundedText),
    UiPasswordSet(RequestId, BoundedText),
    UiPasswordReset(RequestId, BoundedText),
    SessionRevokeAll(RequestId),
    UiOpen,
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
            | CommandKind::SpaceInvite(_, _, _, _)
            | CommandKind::SpaceRedeem(_, _)
            | CommandKind::SpaceRevoke(_, _, _)
            | CommandKind::ControlSyncStatus(_)
            | CommandKind::ControlSyncTrigger(_, _)
            | CommandKind::PrivateRelayConfigure(_, _)
            | CommandKind::PrivateRelayDisable(_)
            | CommandKind::PrivateRelayStatus
            | CommandKind::PublicRelayConfigure(_, _)
            | CommandKind::PublicRelayDisable(_)
            | CommandKind::PublicRelayStatus
            | CommandKind::EchoCall(_, _, _)
            | CommandKind::SnapshotFetch
            | CommandKind::UiOpen
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

    /// Creates an authoritative snapshot query.
    pub const fn snapshot_fetch() -> Self {
        Self {
            kind: CommandKind::SnapshotFetch,
        }
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
            CommandKind::SpaceInvite(_, _, _, _) => "space_invite",
            CommandKind::SpaceRedeem(_, _) => "space_redeem",
            CommandKind::SpaceRevoke(_, _, _) => "space_revoke",
            CommandKind::ControlSyncStatus(_) => "control_sync_status",
            CommandKind::ControlSyncTrigger(_, _) => "control_sync_trigger",
            CommandKind::PrivateRelayConfigure(_, _) => "private_relay_configure",
            CommandKind::PrivateRelayDisable(_) => "private_relay_disable",
            CommandKind::PrivateRelayStatus => "private_relay_status",
            CommandKind::PublicRelayConfigure(_, _) => "public_relay_configure",
            CommandKind::PublicRelayDisable(_) => "public_relay_disable",
            CommandKind::PublicRelayStatus => "public_relay_status",
            CommandKind::EchoCall(_, _, _) => "echo_call",
            CommandKind::UiPasswordSet(_, _) => "ui_password_set",
            CommandKind::UiPasswordReset(_, _) => "ui_password_reset",
            CommandKind::SessionRevokeAll(_) => "session_revoke_all",
            CommandKind::UiOpen => "ui_open",
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
            | CommandKind::SnapshotFetch
            | CommandKind::UiOpen => None,
            CommandKind::SpaceCreate(id, _)
            | CommandKind::SpaceInvite(id, _, _, _)
            | CommandKind::SpaceRedeem(id, _)
            | CommandKind::SpaceRevoke(id, _, _)
            | CommandKind::ControlSyncTrigger(id, _)
            | CommandKind::PrivateRelayConfigure(id, _)
            | CommandKind::PrivateRelayDisable(id)
            | CommandKind::PublicRelayConfigure(id, _)
            | CommandKind::PublicRelayDisable(id)
            | CommandKind::EchoCall(id, _, _)
            | CommandKind::UiPasswordSet(id, _)
            | CommandKind::UiPasswordReset(id, _)
            | CommandKind::SessionRevokeAll(id)
            | CommandKind::GracefulShutdown(id) => Some(id),
        }
    }
}
