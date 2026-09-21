use serde_json::{Value, json};

use super::{
    codec_fields::encode_hex,
    responses::{CommandResult, ResultKind},
    result_data::{
        echo_reply_value, handshake_value, private_relay_value, public_relay_value, status_value,
        ui_auth_result_value, ui_status_value,
    },
    snapshot::{endpoint_value, space_value},
};

pub(super) fn result_value(result: &CommandResult) -> Value {
    let payload = match &result.0 {
        ResultKind::Handshake(value) => handshake_value(value),
        ResultKind::Status(value) => status_value(*value),
        ResultKind::EndpointInfo(value) => endpoint_value(value),
        ResultKind::SpaceCreated(value)
        | ResultKind::Space(value)
        | ResultKind::SpaceInvitationCreated(value)
        | ResultKind::SpaceRedeemed(value)
        | ResultKind::SpaceRevoked(value) => space_value(value),
        ResultKind::Spaces(values) => Value::Array(values.iter().map(space_value).collect()),
        ResultKind::ControlSyncStatus(value) | ResultKind::ControlSyncTriggered(value) => {
            json!({
                "peer_endpoint_ids": value.peers.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>(),
            })
        }
        ResultKind::PrivateRelayConfigured(value) | ResultKind::PrivateRelayStatus(value) => {
            private_relay_value(value)
        }
        ResultKind::PublicRelayConfigured(value) | ResultKind::PublicRelayStatus(value) => {
            public_relay_value(value)
        }
        ResultKind::Echo(value) => echo_reply_value(value),
        ResultKind::UiInitialized(value)
        | ResultKind::UiPasswordSet(value)
        | ResultKind::UiPasswordReset(value)
        | ResultKind::SessionsRevoked(value) => ui_auth_result_value(value),
        ResultKind::UiStatus(value) => ui_status_value(value),
        ResultKind::SpaceDetails(value) => value.to_value(),
        ResultKind::SnapshotStamp(value) => value.to_value(),
        ResultKind::Snapshot(value) => value.to_value(),
        ResultKind::ShuttingDown => json!({}),
    };
    json!({"type": result.result_type(), "payload": payload})
}
