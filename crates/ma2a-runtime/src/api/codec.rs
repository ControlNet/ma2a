use serde_json::{Value, json};

use super::{
    ApiError, LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES,
    codec_fields::{encode_hex, number},
    commands::{Command, CommandKind},
    strict_json,
};

#[path = "codec/parse.rs"]
mod parse;

pub(crate) fn decode_request(input: &[u8]) -> Result<Command, ApiError> {
    if input.len() > MAX_LOCAL_REQUEST_BYTES {
        return Err(ApiError::invalid_input());
    }
    let document = strict_json::decode_root_object(input)?;
    let object = document.fields;
    let version = number(&object, "version")?;
    if document.duplicate_version {
        return Err(ApiError::invalid_input());
    }
    if version != u64::from(LOCAL_API_VERSION) {
        return Err(ApiError::version_mismatch());
    }
    if document.duplicate_member {
        return Err(ApiError::invalid_input());
    }
    parse::command(&object)
}

pub(crate) fn fingerprint(command: &Command) -> Result<[u8; 32], ApiError> {
    let bytes =
        serde_json::to_vec(&command_value(command)).map_err(|_| ApiError::invalid_input())?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

pub(crate) fn encode_request(command: &Command) -> Result<Vec<u8>, ApiError> {
    let mut value = command_value(command);
    let object = value.as_object_mut().ok_or_else(ApiError::invalid_input)?;
    object.insert("version".to_owned(), json!(LOCAL_API_VERSION));
    let encoded = serde_json::to_vec(&value).map_err(|_| ApiError::invalid_input())?;
    if encoded.len() > MAX_LOCAL_REQUEST_BYTES {
        Err(ApiError::invalid_input())
    } else {
        Ok(encoded)
    }
}

fn command_value(command: &Command) -> Value {
    let operation = command.operation();
    match &command.kind {
        CommandKind::Handshake
        | CommandKind::Status
        | CommandKind::EndpointInfo
        | CommandKind::SpaceList
        | CommandKind::PrivateRelayStatus
        | CommandKind::PublicRelayStatus
        | CommandKind::UiStop
        | CommandKind::UiStatus
        | CommandKind::SnapshotStamp
        | CommandKind::SnapshotFetch => json!({"operation": operation}),
        CommandKind::UiStart(host, port) => {
            json!({"operation": operation, "host": host.as_str(), "port": port})
        }
        CommandKind::SpaceCreate(id, name) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "name": name.as_str()})
        }
        CommandKind::SpaceShow(space) | CommandKind::SpaceDetailsFetch(space) => {
            json!({"operation": operation, "space_id": encode_hex(space.as_bytes())})
        }
        CommandKind::SpaceLeave(id, space) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "space_id": encode_hex(space.as_bytes())})
        }
        CommandKind::SpaceRevoke(id, space, peer) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "space_id": encode_hex(space.as_bytes()), "peer_endpoint_id": encode_hex(peer.as_bytes())})
        }
        CommandKind::SpaceInvite(id, space, ttl_ms, output_path) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "space_id": encode_hex(space.as_bytes()), "ttl_ms": ttl_ms, "output_path": output_path.as_str()})
        }
        CommandKind::SpaceRedeem(id, invitation) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "invitation": invitation.as_str()})
        }
        CommandKind::ControlSyncStatus(peer) => {
            json!({"operation": operation, "peer_endpoint_id": encode_hex(peer.as_bytes())})
        }
        CommandKind::ControlSyncTrigger(id, peer) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "peer_endpoint_id": encode_hex(peer.as_bytes())})
        }
        CommandKind::PrivateRelayConfigure(id, config) => {
            super::codec_relay::private_value(operation, *id, config)
        }
        CommandKind::PublicRelayConfigure(id, url) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "url": url.as_str()})
        }
        CommandKind::EchoCall(id, target, payload) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "target_endpoint_id": encode_hex(target.as_bytes()), "payload": payload.as_str()})
        }
        CommandKind::UiInit(id, password)
        | CommandKind::UiPasswordSet(id, password)
        | CommandKind::UiPasswordReset(id, password) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "password": password.as_str()})
        }
        CommandKind::PrivateRelayDisable(id)
        | CommandKind::PublicRelayDisable(id)
        | CommandKind::SessionRevokeAll(id)
        | CommandKind::GracefulShutdown(id) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes())})
        }
    }
}
