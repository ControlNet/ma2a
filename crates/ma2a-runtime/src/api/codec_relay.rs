use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_fields::{bounded_text, encode_hex, exact_fields, request_id, space_ids, text},
    commands::{BoundedText, CommandKind, PrivateRelayConfiguration, PrivateRelayMode},
};

pub(super) fn private_configure(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &[
            "version",
            "operation",
            "request_id",
            "mode",
            "listen",
            "public_url",
            "served_space_ids",
            "certificate_path",
            "private_key_path",
        ],
    )?;
    let mode = match text(object, "mode")? {
        "native_tls" => PrivateRelayMode::NativeTls,
        "external_termination" => PrivateRelayMode::ExternalTermination,
        _ => return Err(ApiError::invalid_input()),
    };
    Ok(CommandKind::PrivateRelayConfigure(
        request_id(object, "request_id")?,
        PrivateRelayConfiguration {
            mode,
            listen: bounded_text(object, "listen", 128)?,
            public_url: bounded_text(object, "public_url", 2_048)?,
            served_spaces: space_ids(object, "served_space_ids")?,
            certificate_path: optional_text(object, "certificate_path", 4_096)?,
            private_key_path: optional_text(object, "private_key_path", 4_096)?,
        },
    ))
}

pub(super) fn public_configure(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
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

pub(super) fn request_only(
    object: &Map<String, Value>,
    constructor: fn(ma2a_core::RequestId) -> CommandKind,
) -> Result<CommandKind, ApiError> {
    exact_fields(object, &["version", "operation", "request_id"])?;
    Ok(constructor(request_id(object, "request_id")?))
}

pub(super) fn private_value(
    operation: &str,
    id: ma2a_core::RequestId,
    config: &PrivateRelayConfiguration,
) -> Value {
    json!({
        "operation": operation,
        "request_id": encode_hex(id.as_bytes()),
        "mode": match config.mode {
            PrivateRelayMode::NativeTls => "native_tls",
            PrivateRelayMode::ExternalTermination => "external_termination",
        },
        "listen": config.listen.as_str(),
        "public_url": config.public_url.as_str(),
        "served_space_ids": config.served_spaces.iter().map(|space| encode_hex(space.as_bytes())).collect::<Vec<_>>(),
        "certificate_path": config.certificate_path.as_ref().map(BoundedText::as_str),
        "private_key_path": config.private_key_path.as_ref().map(BoundedText::as_str),
    })
}

fn optional_text(
    object: &Map<String, Value>,
    field: &str,
    maximum: usize,
) -> Result<Option<BoundedText>, ApiError> {
    match object.get(field) {
        Some(Value::String(value)) => BoundedText::parse(value, maximum).map(Some),
        Some(Value::Null) => Ok(None),
        Some(_) | None => Err(ApiError::invalid_input()),
    }
}
