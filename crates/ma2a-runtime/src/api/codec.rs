use serde_json::{Map, Value, json};

use super::{
    ApiError, LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES,
    codec_fields::{
        bounded_text, encode_hex, endpoint_id, exact_fields, number, request_id, space_id, text,
    },
    commands::{Command, CommandKind},
    strict_json,
};

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
    parse_command(&object)
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

fn parse_command(object: &Map<String, Value>) -> Result<Command, ApiError> {
    let operation = text(object, "operation")?;
    let kind = match operation {
        "handshake" => unit(object, CommandKind::Handshake)?,
        "status" => unit(object, CommandKind::Status)?,
        "endpoint_info" => unit(object, CommandKind::EndpointInfo)?,
        "space_create" => space_create(object)?,
        "space_list" => unit(object, CommandKind::SpaceList)?,
        "space_show" => space_show(object)?,
        "space_invite" => space_invite(object)?,
        "space_redeem" => space_redeem(object)?,
        "space_revoke" => space_revoke(object)?,
        "control_sync_status" => control_sync_status(object)?,
        "control_sync_trigger" => control_sync_trigger(object)?,
        "private_relay_configure" => super::codec_relay::private_configure(object)?,
        "private_relay_disable" => {
            super::codec_relay::request_only(object, CommandKind::PrivateRelayDisable)?
        }
        "private_relay_status" => unit(object, CommandKind::PrivateRelayStatus)?,
        "public_relay_configure" => super::codec_relay::public_configure(object)?,
        "public_relay_disable" => {
            super::codec_relay::request_only(object, CommandKind::PublicRelayDisable)?
        }
        "public_relay_status" => unit(object, CommandKind::PublicRelayStatus)?,
        "echo_call" => echo_call(object)?,
        "ui_password_set" => ui_password(object, true)?,
        "ui_password_reset" => ui_password(object, false)?,
        "session_revoke_all" => request_only(object, false)?,
        "ui_open" => unit(object, CommandKind::UiOpen)?,
        "snapshot_stamp" => unit(object, CommandKind::SnapshotStamp)?,
        "space_details_fetch" => {
            exact_fields(object, &["version", "operation", "space_id"])?;
            CommandKind::SpaceDetailsFetch(space_id(object, "space_id")?)
        }
        "snapshot_fetch" => unit(object, CommandKind::SnapshotFetch)?,
        "graceful_shutdown" => request_only(object, true)?,
        _ => return Err(ApiError::invalid_input()),
    };
    Ok(Command { kind })
}

fn unit(object: &Map<String, Value>, kind: CommandKind) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation"])?;
    Ok(kind)
}

fn space_create(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id", "name"])?;
    Ok(CommandKind::SpaceCreate(
        request_id(object, "request_id")?,
        bounded_text(object, "name", 128)?,
    ))
}

fn space_show(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "space_id"])?;
    Ok(CommandKind::SpaceShow(space_id(object, "space_id")?))
}

fn space_revoke(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &[
            "version",
            "operation",
            "request_id",
            "space_id",
            "peer_endpoint_id",
        ],
    )?;
    let id = request_id(object, "request_id")?;
    let space = space_id(object, "space_id")?;
    let peer = endpoint_id(object, "peer_endpoint_id")?;
    Ok(CommandKind::SpaceRevoke(id, space, peer))
}

fn space_invite(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &[
            "version",
            "operation",
            "request_id",
            "space_id",
            "ttl_ms",
            "output_path",
        ],
    )?;
    Ok(CommandKind::SpaceInvite(
        request_id(object, "request_id")?,
        space_id(object, "space_id")?,
        number(object, "ttl_ms")?,
        bounded_text(object, "output_path", 4_096)?,
    ))
}

fn space_redeem(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &["version", "operation", "request_id", "invitation"],
    )?;
    Ok(CommandKind::SpaceRedeem(
        request_id(object, "request_id")?,
        bounded_text(object, "invitation", 2_048)?,
    ))
}

fn control_sync_status(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "peer_endpoint_id"])?;
    Ok(CommandKind::ControlSyncStatus(endpoint_id(
        object,
        "peer_endpoint_id",
    )?))
}

fn control_sync_trigger(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &["version", "operation", "request_id", "peer_endpoint_id"],
    )?;
    Ok(CommandKind::ControlSyncTrigger(
        request_id(object, "request_id")?,
        endpoint_id(object, "peer_endpoint_id")?,
    ))
}

fn echo_call(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &[
            "version",
            "operation",
            "request_id",
            "target_endpoint_id",
            "payload",
        ],
    )?;
    Ok(CommandKind::EchoCall(
        request_id(object, "request_id")?,
        endpoint_id(object, "target_endpoint_id")?,
        bounded_text(object, "payload", 4_096)?,
    ))
}

fn ui_password(object: &Map<String, Value>, set: bool) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id", "password"])?;
    let id = request_id(object, "request_id")?;
    let password = bounded_text(object, "password", 1_024)?;
    if set {
        Ok(CommandKind::UiPasswordSet(id, password))
    } else {
        Ok(CommandKind::UiPasswordReset(id, password))
    }
}

fn request_only(object: &Map<String, Value>, shutdown: bool) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id"])?;
    let id = request_id(object, "request_id")?;
    if shutdown {
        Ok(CommandKind::GracefulShutdown(id))
    } else {
        Ok(CommandKind::SessionRevokeAll(id))
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
        | CommandKind::UiOpen
        | CommandKind::SnapshotStamp
        | CommandKind::SnapshotFetch => json!({"operation": operation}),
        CommandKind::SpaceCreate(id, name) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "name": name.as_str()})
        }
        CommandKind::SpaceShow(space) | CommandKind::SpaceDetailsFetch(space) => {
            json!({"operation": operation, "space_id": encode_hex(space.as_bytes())})
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
        CommandKind::UiPasswordSet(id, password) | CommandKind::UiPasswordReset(id, password) => {
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
