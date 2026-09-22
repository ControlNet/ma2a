use serde_json::{Map, Value};

use crate::api::{
    ApiError,
    codec_fields::{bounded_text, endpoint_id, exact_fields, number, request_id, space_id, text},
    commands::{Command, CommandKind},
};

pub(super) fn command(object: &Map<String, Value>) -> Result<Command, ApiError> {
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
        "space_leave" => space_leave(object)?,
        "control_sync_status" => control_sync_status(object)?,
        "control_sync_trigger" => control_sync_trigger(object)?,
        "private_relay_configure" => crate::api::codec_relay::private_configure(object)?,
        "private_relay_disable" => {
            crate::api::codec_relay::request_only(object, CommandKind::PrivateRelayDisable)?
        }
        "private_relay_status" => unit(object, CommandKind::PrivateRelayStatus)?,
        "public_relay_configure" => crate::api::codec_relay::public_configure(object)?,
        "public_relay_disable" => {
            crate::api::codec_relay::request_only(object, CommandKind::PublicRelayDisable)?
        }
        "public_relay_status" => unit(object, CommandKind::PublicRelayStatus)?,
        "echo_call" => echo_call(object)?,
        "ui_password_set" => ui_password(object, true)?,
        "ui_password_reset" => ui_password(object, false)?,
        "session_revoke_all" => request_only(object, false)?,
        "ui_init" => {
            exact_fields(object, &["version", "operation", "request_id", "password"])?;
            CommandKind::UiInit(
                request_id(object, "request_id")?,
                bounded_text(object, "password", 1_024)?,
            )
        }
        "ui_start" => ui_start(object)?,
        "ui_stop" => unit(object, CommandKind::UiStop)?,
        "ui_status" => unit(object, CommandKind::UiStatus)?,
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
    Ok(CommandKind::SpaceRevoke(
        request_id(object, "request_id")?,
        space_id(object, "space_id")?,
        endpoint_id(object, "peer_endpoint_id")?,
    ))
}

fn space_leave(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id", "space_id"])?;
    Ok(CommandKind::SpaceLeave(
        request_id(object, "request_id")?,
        space_id(object, "space_id")?,
    ))
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

fn ui_start(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "host", "port"])?;
    let host = bounded_text(object, "host", 253)?;
    if host.as_str().parse::<std::net::IpAddr>().is_err()
        && !host
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(ApiError::invalid_input());
    }
    let port = u16::try_from(number(object, "port")?).map_err(|_| ApiError::invalid_input())?;
    Ok(CommandKind::UiStart(host, port))
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
