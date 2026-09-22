use ma2a_core::{RequestId, SpaceId};
use zeroize::Zeroizing;

use super::{ApiError, BoundedText, Command, CommandKind, UiControlCommand};

impl Command {
    pub(crate) fn into_ui_control(self) -> Result<UiControlCommand, ApiError> {
        match self.kind {
            CommandKind::UiInit(_, password) => {
                Ok(UiControlCommand::PasswordInit(password.into_secret()))
            }
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
            | CommandKind::SpaceDetailsFetch(_)
            | CommandKind::SnapshotStamp
            | CommandKind::SnapshotFetch
            | CommandKind::UiStart(_, _)
            | CommandKind::UiStop
            | CommandKind::UiStatus
            | CommandKind::GracefulShutdown(_) => Err(ApiError::invalid_input()),
        }
    }

    /// Creates an atomic password initialization or reset command.
    ///
    /// # Errors
    /// Returns an error if randomness or password bounds fail.
    pub fn ui_init(password: Zeroizing<String>) -> Result<Self, ApiError> {
        Ok(Self {
            kind: CommandKind::UiInit(
                RequestId::random().map_err(ApiError::new)?,
                BoundedText::parse_secret(password, 1_024)?,
            ),
        })
    }

    /// Returns whether a command controls volatile Web UI state without durable replay.
    pub const fn is_ui_lifecycle(&self) -> bool {
        matches!(
            self.kind,
            CommandKind::UiStart(_, _) | CommandKind::UiStop | CommandKind::UiStatus
        )
    }

    pub(crate) fn ui_binding(&self) -> Option<(&str, u16)> {
        match &self.kind {
            CommandKind::UiStart(host, port) => Some((host.as_str(), *port)),
            _ => None,
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

    /// Creates a complete Space detail query.
    pub const fn space_details_fetch(id: SpaceId) -> Self {
        Self {
            kind: CommandKind::SpaceDetailsFetch(id),
        }
    }

    /// Creates a lightweight durable revision query.
    pub const fn snapshot_stamp() -> Self {
        Self {
            kind: CommandKind::SnapshotStamp,
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
            CommandKind::UiInit(_, _) => "ui_init",
            CommandKind::UiStart(_, _) => "ui_start",
            CommandKind::UiStop => "ui_stop",
            CommandKind::UiStatus => "ui_status",
            CommandKind::SpaceDetailsFetch(_) => "space_details_fetch",
            CommandKind::SnapshotStamp => "snapshot_stamp",
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
            | CommandKind::SpaceDetailsFetch(_)
            | CommandKind::SnapshotStamp
            | CommandKind::SnapshotFetch
            | CommandKind::UiStart(_, _)
            | CommandKind::UiStop
            | CommandKind::UiStatus => None,
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
            | CommandKind::UiInit(id, _)
            | CommandKind::UiPasswordSet(id, _)
            | CommandKind::UiPasswordReset(id, _)
            | CommandKind::SessionRevokeAll(id)
            | CommandKind::GracefulShutdown(id) => Some(id),
        }
    }
}
