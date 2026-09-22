//! Closed local Runtime command set.

use ma2a_core::{EndpointId, RequestId, SpaceId};
use zeroize::Zeroizing;

use super::{ApiError, MAX_TEXT_BYTES};

#[path = "command_accessors.rs"]
mod accessors;
#[path = "command_methods.rs"]
mod methods;

/// Exact ordered operation inventory carried by the schema and TypeScript contract.
pub const COMMAND_NAMES: [&str; 30] = [
    "handshake",
    "status",
    "endpoint_info",
    "space_create",
    "space_list",
    "space_show",
    "space_invite",
    "space_redeem",
    "space_revoke",
    "space_leave",
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
    "ui_init",
    "ui_start",
    "ui_stop",
    "ui_status",
    "snapshot_fetch",
    "space_details_fetch",
    "snapshot_stamp",
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
    PasswordInit(Zeroizing<String>),
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
    SpaceLeave(RequestId, SpaceId),
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
    UiInit(RequestId, BoundedText),
    UiStart(BoundedText, u16),
    UiStop,
    UiStatus,
    SnapshotFetch,
    SpaceDetailsFetch(SpaceId),
    SnapshotStamp,
    GracefulShutdown(RequestId),
}

/// One parsed member of the exact local API v1 command set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub(crate) kind: CommandKind,
}
