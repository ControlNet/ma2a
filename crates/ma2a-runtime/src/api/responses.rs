//! Closed bounded local API response contract.

use ma2a_core::{ProtocolError, RequestId};
use serde_json::{Value, json};

use super::{
    ApiError, LOCAL_API_VERSION, MAX_LOCAL_RESPONSE_BYTES,
    codec_fields::encode_hex,
    result_data::{
        EchoReplyView, HandshakeView, PrivateRelayView, PublicRelayView, RuntimeStatusView,
        UiStatusView,
    },
    snapshot::{
        ControlSyncView, EndpointView, RuntimeSnapshot, SpaceIdentityView, SpaceView, UiAuthView,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResultKind {
    Handshake(HandshakeView),
    Status(RuntimeStatusView),
    EndpointInfo(EndpointView),
    SpaceCreated(SpaceView),
    Spaces(Vec<SpaceView>),
    Space(SpaceView),
    SpaceInvitationCreated(SpaceView),
    SpaceRedeemed(SpaceView),
    SpaceRevoked(SpaceView),
    SpaceLeft(SpaceIdentityView),
    ControlSyncStatus(ControlSyncView),
    ControlSyncTriggered(ControlSyncView),
    PrivateRelayConfigured(PrivateRelayView),
    PrivateRelayStatus(PrivateRelayView),
    PublicRelayConfigured(PublicRelayView),
    PublicRelayStatus(PublicRelayView),
    Echo(EchoReplyView),
    UiInitialized(UiAuthView),
    UiPasswordSet(UiAuthView),
    UiPasswordReset(UiAuthView),
    SessionsRevoked(UiAuthView),
    UiStatus(UiStatusView),
    Snapshot(RuntimeSnapshot),
    SpaceDetails(super::SpaceDetailsView),
    SnapshotStamp(super::SnapshotStampView),
    ShuttingDown,
}

/// One member of the exact local API v1 result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult(pub(super) ResultKind, Option<u64>);

/// Typed UI credential result returned through current-user local control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UiControlResult<'a> {
    /// Password established or reset.
    Initialized(&'a UiAuthView),
    /// The first UI password verifier was established.
    PasswordSet(&'a UiAuthView),
    /// The UI password verifier was replaced and older sessions were revoked.
    PasswordReset(&'a UiAuthView),
    /// Every browser session was revoked.
    SessionsRevoked(&'a UiAuthView),
}

impl CommandResult {
    pub(crate) const fn at_revision(mut self, revision: u64) -> Self {
        self.1 = Some(revision);
        self
    }

    pub(crate) const fn committed_revision(&self) -> Option<u64> {
        self.1
    }

    /// Returns the typed UI credential result, when this result belongs to that command family.
    #[must_use]
    pub const fn ui_control_result(&self) -> Option<UiControlResult<'_>> {
        match &self.0 {
            ResultKind::UiInitialized(value) => Some(UiControlResult::Initialized(value)),
            ResultKind::UiPasswordSet(value) => Some(UiControlResult::PasswordSet(value)),
            ResultKind::UiPasswordReset(value) => Some(UiControlResult::PasswordReset(value)),
            ResultKind::SessionsRevoked(value) => Some(UiControlResult::SessionsRevoked(value)),
            ResultKind::Handshake(_)
            | ResultKind::Status(_)
            | ResultKind::EndpointInfo(_)
            | ResultKind::SpaceCreated(_)
            | ResultKind::Spaces(_)
            | ResultKind::Space(_)
            | ResultKind::SpaceInvitationCreated(_)
            | ResultKind::SpaceRedeemed(_)
            | ResultKind::SpaceRevoked(_)
            | ResultKind::SpaceLeft(_)
            | ResultKind::ControlSyncStatus(_)
            | ResultKind::ControlSyncTriggered(_)
            | ResultKind::PrivateRelayConfigured(_)
            | ResultKind::PrivateRelayStatus(_)
            | ResultKind::PublicRelayConfigured(_)
            | ResultKind::PublicRelayStatus(_)
            | ResultKind::Echo(_)
            | ResultKind::UiStatus(_)
            | ResultKind::SpaceDetails(_)
            | ResultKind::SnapshotStamp(_)
            | ResultKind::Snapshot(_)
            | ResultKind::ShuttingDown => None,
        }
    }

    /// Creates the graceful-shutdown acknowledgement.
    pub const fn shutting_down() -> Self {
        Self(ResultKind::ShuttingDown, None)
    }

    /// Creates the pre-authorization handshake result.
    pub const fn handshake(value: HandshakeView) -> Self {
        Self(ResultKind::Handshake(value), None)
    }

    /// Creates a complete Space detail result.
    pub const fn space_details(value: super::SpaceDetailsView) -> Self {
        Self(ResultKind::SpaceDetails(value), None)
    }
    /// Creates a lightweight revision stamp result.
    pub const fn snapshot_stamp(value: super::SnapshotStampView) -> Self {
        Self(ResultKind::SnapshotStamp(value), None)
    }
    /// Creates the authoritative snapshot result.
    pub const fn snapshot(value: RuntimeSnapshot) -> Self {
        Self(ResultKind::Snapshot(value), None)
    }

    /// Creates a Runtime status result.
    pub const fn status(value: RuntimeStatusView) -> Self {
        Self(ResultKind::Status(value), None)
    }
    /// Creates an Endpoint information result.
    pub const fn endpoint_info(value: EndpointView) -> Self {
        Self(ResultKind::EndpointInfo(value), None)
    }
    /// Creates a Space creation result.
    pub const fn space_created(value: SpaceView) -> Self {
        Self(ResultKind::SpaceCreated(value), None)
    }
    /// Creates a bounded Space list result.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 Spaces are supplied.
    pub fn spaces(value: Vec<SpaceView>) -> Result<Self, ApiError> {
        if value.len() > super::MAX_COLLECTION_ITEMS {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self(ResultKind::Spaces(value), None))
        }
    }
    /// Creates a Space detail result.
    pub const fn space(value: SpaceView) -> Self {
        Self(ResultKind::Space(value), None)
    }
    /// Creates a Space invitation receipt without invite secret material.
    pub const fn space_invitation_created(value: SpaceView) -> Self {
        Self(ResultKind::SpaceInvitationCreated(value), None)
    }
    /// Creates a Space redemption result.
    pub const fn space_redeemed(value: SpaceView) -> Self {
        Self(ResultKind::SpaceRedeemed(value), None)
    }
    /// Creates a Space revocation result.
    pub const fn space_revoked(value: SpaceView) -> Self {
        Self(ResultKind::SpaceRevoked(value), None)
    }
    /// Creates a Space departure result for the leaving member.
    ///
    /// It carries only stable identity, because after departure this Endpoint no
    /// longer holds authoritative membership state for that Space.
    pub const fn space_left(value: SpaceIdentityView) -> Self {
        Self(ResultKind::SpaceLeft(value), None)
    }
    /// Creates a control-sync status result.
    pub const fn control_sync_status(value: ControlSyncView) -> Self {
        Self(ResultKind::ControlSyncStatus(value), None)
    }
    /// Creates a control-sync trigger result.
    pub const fn control_sync_triggered(value: ControlSyncView) -> Self {
        Self(ResultKind::ControlSyncTriggered(value), None)
    }
    /// Creates a Private Relay configuration result.
    pub const fn private_relay_configured(value: PrivateRelayView) -> Self {
        Self(ResultKind::PrivateRelayConfigured(value), None)
    }
    /// Creates a Private Relay status result.
    pub const fn private_relay_status(value: PrivateRelayView) -> Self {
        Self(ResultKind::PrivateRelayStatus(value), None)
    }
    /// Creates a Public Relay configuration result.
    pub const fn public_relay_configured(value: PublicRelayView) -> Self {
        Self(ResultKind::PublicRelayConfigured(value), None)
    }
    /// Creates a Public Relay status result.
    pub const fn public_relay_status(value: PublicRelayView) -> Self {
        Self(ResultKind::PublicRelayStatus(value), None)
    }
    /// Creates an Echo result.
    pub const fn echo(value: EchoReplyView) -> Self {
        Self(ResultKind::Echo(value), None)
    }
    /// Reports successful password initialization or reset.
    pub const fn ui_initialized(value: UiAuthView) -> Self {
        Self(ResultKind::UiInitialized(value), None)
    }

    /// Reports successful initial password setup.
    pub const fn ui_password_set(value: UiAuthView) -> Self {
        Self(ResultKind::UiPasswordSet(value), None)
    }
    /// Creates a UI password-reset result.
    pub const fn ui_password_reset(value: UiAuthView) -> Self {
        Self(ResultKind::UiPasswordReset(value), None)
    }
    /// Creates a session revoke-all result.
    pub const fn sessions_revoked(value: UiAuthView) -> Self {
        Self(ResultKind::SessionsRevoked(value), None)
    }
    /// Creates a daemon-owned Web UI lifecycle status result.
    pub const fn ui_status(value: UiStatusView) -> Self {
        Self(ResultKind::UiStatus(value), None)
    }

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
        }
    }
}

/// Successful response carrying an authoritative post-command revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    version: u16,
    request_id: Option<RequestId>,
    revision: u64,
    result: CommandResult,
}

impl ApiResponse {
    pub(crate) const fn new(
        request_id: Option<RequestId>,
        revision: u64,
        result: CommandResult,
    ) -> Self {
        Self {
            version: LOCAL_API_VERSION,
            request_id,
            revision,
            result,
        }
    }

    /// Returns the authoritative revision after execution or replay.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the mutation correlation identifier when present.
    pub const fn request_id(&self) -> Option<RequestId> {
        self.request_id
    }

    pub(crate) const fn result_type(&self) -> &'static str {
        self.result.result_type()
    }
}

/// Serializes one successful response and enforces the response bound.
///
/// # Errors
/// Returns an internal error when serialization fails or exceeds the response bound.
pub fn encode_response(response: &ApiResponse) -> Result<Vec<u8>, ApiError> {
    let value = json!({
        "version": response.version,
        "request_id": response.request_id.map(|id| encode_hex(id.as_bytes())),
        "revision": response.revision,
        "result": super::response_value::result_value(&response.result),
    });
    bounded_json(&value)
}

/// Serializes one typed error without reading Runtime state.
///
/// # Errors
/// Returns an internal error when serialization fails or exceeds the response bound.
pub fn encode_error(error: ApiError) -> Result<Vec<u8>, ApiError> {
    let value = json!({
        "version": LOCAL_API_VERSION,
        "error": error.code().name(),
        "remediation": error.remediation(),
    });
    bounded_json(&value)
}

fn bounded_json(value: &Value) -> Result<Vec<u8>, ApiError> {
    let encoded = serde_json::to_vec(value).map_err(|_| ApiError::new(ProtocolError::INTERNAL))?;
    if encoded.len() > MAX_LOCAL_RESPONSE_BYTES {
        Err(ApiError::new(ProtocolError::INTERNAL))
    } else {
        Ok(encoded)
    }
}
