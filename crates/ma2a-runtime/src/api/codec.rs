use serde_json::{Map, Value, json};

use super::{
    ApiError, LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES,
    codec_fields::{
        bounded_text, encode_hex, endpoint_id, exact_fields, number, object, request_id, space_id,
        text,
    },
    commands::{Command, CommandKind, PrivateRelayMode},
};

pub(crate) fn decode_request(input: &[u8]) -> Result<Command, ApiError> {
    if input.len() > MAX_LOCAL_REQUEST_BYTES {
        return Err(ApiError::invalid_input());
    }
    let document = serde_json::from_slice::<Value>(input).map_err(|_| ApiError::invalid_input())?;
    let object = object(&document)?;
    let version = number(&object, "version")?;
    if version != u64::from(LOCAL_API_VERSION) {
        return Err(ApiError::version_mismatch());
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
        "space_invite" => space_peer(object, true)?,
        "space_redeem" => space_redeem(object)?,
        "space_revoke" => space_peer(object, false)?,
        "control_sync_status" => control_sync_status(object)?,
        "control_sync_trigger" => control_sync_trigger(object)?,
        "private_relay_configure" => private_relay_configure(object)?,
        "private_relay_status" => unit(object, CommandKind::PrivateRelayStatus)?,
        "public_relay_configure" => public_relay_configure(object)?,
        "public_relay_status" => unit(object, CommandKind::PublicRelayStatus)?,
        "echo_call" => echo_call(object)?,
        "ui_password_set" => ui_password(object, true)?,
        "ui_password_reset" => ui_password(object, false)?,
        "session_revoke_all" => request_only(object, false)?,
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

fn space_peer(object: &Map<String, Value>, invite: bool) -> Result<CommandKind, ApiError> {
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
    if invite {
        Ok(CommandKind::SpaceInvite(id, space, peer))
    } else {
        Ok(CommandKind::SpaceRevoke(id, space, peer))
    }
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

fn private_relay_configure(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &["version", "operation", "request_id", "mode", "host", "port"],
    )?;
    let mode = match text(object, "mode")? {
        "native_tls" => PrivateRelayMode::NativeTls,
        "external_termination" => PrivateRelayMode::ExternalTermination,
        _ => return Err(ApiError::invalid_input()),
    };
    let port = u16::try_from(number(object, "port")?).map_err(|_| ApiError::invalid_input())?;
    Ok(CommandKind::PrivateRelayConfigure(
        request_id(object, "request_id")?,
        mode,
        bounded_text(object, "host", 253)?,
        port,
    ))
}

fn public_relay_configure(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id", "url"])?;
    let url = bounded_text(object, "url", 2_048)?;
    let authority = url
        .as_str()
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(ApiError::invalid_input)?;
    if authority.contains('@') {
        return Err(ApiError::invalid_input());
    }
    Ok(CommandKind::PublicRelayConfigure(
        request_id(object, "request_id")?,
        url,
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
        | CommandKind::SnapshotFetch => json!({"operation": operation}),
        CommandKind::SpaceCreate(id, name) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "name": name.as_str()})
        }
        CommandKind::SpaceShow(space) => {
            json!({"operation": operation, "space_id": encode_hex(space.as_bytes())})
        }
        CommandKind::SpaceInvite(id, space, peer) | CommandKind::SpaceRevoke(id, space, peer) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "space_id": encode_hex(space.as_bytes()), "peer_endpoint_id": encode_hex(peer.as_bytes())})
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
        CommandKind::PrivateRelayConfigure(id, mode, host, port) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes()), "mode": match mode { PrivateRelayMode::NativeTls => "native_tls", PrivateRelayMode::ExternalTermination => "external_termination" }, "host": host.as_str(), "port": port})
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
        CommandKind::SessionRevokeAll(id) | CommandKind::GracefulShutdown(id) => {
            json!({"operation": operation, "request_id": encode_hex(id.as_bytes())})
        }
    }
}
