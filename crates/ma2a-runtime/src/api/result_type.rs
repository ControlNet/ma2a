//! Exact result discriminants of the local API v1 result set.

use super::responses::{CommandResult, ResultKind};

impl CommandResult {
    /// Returns the exact result discriminant.
    pub const fn result_type(&self) -> &'static str {
        match self.0 {
            ResultKind::Handshake(_) => "handshake",
            ResultKind::Status(_) => "status",
            ResultKind::EndpointInfo(_) => "endpoint_info",
            ResultKind::SpaceCreated(_) => "space_created",
            ResultKind::Spaces(_) => "spaces",
            ResultKind::Space(_) => "space",
            ResultKind::SpaceInvitationCreated(_) => "space_invitation_created",
            ResultKind::SpaceRedeemed(_) => "space_redeemed",
            ResultKind::SpaceRevoked(_) => "space_revoked",
            ResultKind::SpaceLeft(_) => "space_left",
            ResultKind::ControlSyncStatus(_) => "control_sync_status",
            ResultKind::ControlSyncTriggered(_) => "control_sync_triggered",
            ResultKind::PrivateRelayConfigured(_) => "private_relay_configured",
            ResultKind::PrivateRelayStatus(_) => "private_relay_status",
            ResultKind::PublicRelayConfigured(_) => "public_relay_configured",
            ResultKind::PublicRelayStatus(_) => "public_relay_status",
            ResultKind::Echo(_) => "echo",
            ResultKind::UiInitialized(_) => "ui_initialized",
            ResultKind::UiPasswordSet(_) => "ui_password_set",
            ResultKind::UiPasswordReset(_) => "ui_password_reset",
            ResultKind::SessionsRevoked(_) => "sessions_revoked",
            ResultKind::UiStatus(_) => "ui_status",
            ResultKind::SpaceDetails(_) => "space_details",
            ResultKind::SnapshotStamp(_) => "snapshot_stamp",
            ResultKind::Snapshot(_) => "snapshot",
            ResultKind::ShuttingDown => "shutting_down",
            ResultKind::SpaceGranted(_) => "space_granted",
            ResultKind::ServiceExposed(_) => "service_exposed",
            ResultKind::ServiceUnexposed(_) => "service_unexposed",
            ResultKind::ServiceExposures(_) => "service_exposures",
            ResultKind::Operations(_) => "operations",
            ResultKind::StreamOpened => "stream_opened",
            ResultKind::Process(ref result) => result.result_type(),
        }
    }
}
