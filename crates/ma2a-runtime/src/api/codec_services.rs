//! Local API commands for service grants and local exposure.

use ma2a_core::{
    RequestId,
    service::{ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName},
};
use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_fields::{encode_hex, endpoint_id, exact_fields, request_id, space_id},
    commands::{BoundedText, CommandKind},
};

/// Service grant flags in `files_read, files_write, exec, shell, proxy, process` order.
pub(crate) const GRANT_FIELDS: [&str; 6] = [
    "files_read",
    "files_write",
    "exec",
    "shell",
    "proxy",
    "process",
];

/// Most bytes of an exposed root's local directory path.
const MAX_ROOT_PATH_BYTES: usize = 4_096;

/// One exposure named by a command. `local` carries the directory and access of a root
/// being exposed; unexposing a root names it only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExposureSpec {
    FilesRoot {
        root: RootName,
        local: Option<(BoundedText, RootAccess)>,
    },
    Exec,
    Shell,
    Proxy(ProxyRule),
    Process,
}

pub(super) fn stream_open(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    exact_fields(
        object,
        &[
            "version",
            "operation",
            "request_id",
            "target_endpoint_id",
            "service",
        ],
    )?;
    let service = object
        .get("service")
        .and_then(Value::as_str)
        .and_then(ma2a_core::ServiceKind::from_name)
        .filter(|service| *service != ma2a_core::ServiceKind::Echo)
        .ok_or_else(ApiError::invalid_input)?;
    Ok(CommandKind::StreamOpen(
        request_id(object, "request_id")?,
        endpoint_id(object, "target_endpoint_id")?,
        service,
    ))
}

pub(super) fn space_grant(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    let mut fields = vec![
        "version",
        "operation",
        "request_id",
        "space_id",
        "peer_endpoint_id",
    ];
    fields.extend(GRANT_FIELDS);
    exact_fields(object, &fields)?;
    let mut grants = [false; 6];
    for (slot, field) in grants.iter_mut().zip(GRANT_FIELDS) {
        *slot = object
            .get(field)
            .and_then(Value::as_bool)
            .ok_or_else(ApiError::invalid_input)?;
    }
    Ok(CommandKind::SpaceGrant(
        request_id(object, "request_id")?,
        space_id(object, "space_id")?,
        endpoint_id(object, "peer_endpoint_id")?,
        grants,
    ))
}

pub(super) fn service_exposure(
    object: &Map<String, Value>,
    expose: bool,
) -> Result<CommandKind, ApiError> {
    let mut fields = vec![
        "version",
        "operation",
        "request_id",
        "space_id",
        "kind",
        "root",
        "host",
        "port_start",
        "port_end",
    ];
    if expose {
        fields.extend(["path", "access"]);
    }
    exact_fields(object, &fields)?;
    let id = request_id(object, "request_id")?;
    let space = space_id(object, "space_id")?;
    let spec = exposure_spec(object, expose)?;
    Ok(if expose {
        CommandKind::ServiceExpose(id, space, spec)
    } else {
        CommandKind::ServiceUnexpose(id, space, spec)
    })
}

fn exposure_spec(object: &Map<String, Value>, expose: bool) -> Result<ExposureSpec, ApiError> {
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(ApiError::invalid_input)?;
    let root = nullable(object, "root")?;
    let host = nullable(object, "host")?;
    let ports = (
        nullable_port(object, "port_start")?,
        nullable_port(object, "port_end")?,
    );
    let (path, access) = if expose {
        (nullable(object, "path")?, nullable(object, "access")?)
    } else {
        (None, None)
    };
    let root_only = host.is_none() && ports == (None, None);
    let empty = root.is_none() && path.is_none() && access.is_none() && root_only;
    match kind {
        "files_root" if root_only => {
            let root =
                RootName::new(root.ok_or_else(ApiError::invalid_input)?).map_err(ApiError::new)?;
            let local = if expose {
                let path = BoundedText::parse(
                    path.ok_or_else(ApiError::invalid_input)?,
                    MAX_ROOT_PATH_BYTES,
                )?;
                let access = RootAccess::from_name(access.ok_or_else(ApiError::invalid_input)?)
                    .ok_or_else(ApiError::invalid_input)?;
                Some((path, access))
            } else {
                None
            };
            Ok(ExposureSpec::FilesRoot { root, local })
        }
        "exec" if empty => Ok(ExposureSpec::Exec),
        "shell" if empty => Ok(ExposureSpec::Shell),
        "process" if empty => Ok(ExposureSpec::Process),
        "proxy" if root.is_none() && path.is_none() && access.is_none() => {
            let host = match host.ok_or_else(ApiError::invalid_input)? {
                "*" => ProxyHostPattern::Any,
                exact => ProxyHostPattern::Exact(ProxyHost::new(exact).map_err(ApiError::new)?),
            };
            let (Some(start), Some(end)) = ports else {
                return Err(ApiError::invalid_input());
            };
            ProxyRule::new(host, (start, end))
                .map(ExposureSpec::Proxy)
                .map_err(ApiError::new)
        }
        _ => Err(ApiError::invalid_input()),
    }
}

fn nullable<'a>(object: &'a Map<String, Value>, field: &str) -> Result<Option<&'a str>, ApiError> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(Value::Null) => Ok(None),
        Some(_) | None => Err(ApiError::invalid_input()),
    }
}

fn nullable_port(object: &Map<String, Value>, field: &str) -> Result<Option<u16>, ApiError> {
    match object.get(field) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .map(Some)
            .ok_or_else(ApiError::invalid_input),
        None => Err(ApiError::invalid_input()),
    }
}

pub(super) fn grant_value(
    operation: &str,
    (id, space, peer): (RequestId, ma2a_core::SpaceId, ma2a_core::EndpointId),
    grants: [bool; 6],
) -> Value {
    let mut value = json!({
        "operation": operation,
        "request_id": encode_hex(id.as_bytes()),
        "space_id": encode_hex(space.as_bytes()),
        "peer_endpoint_id": encode_hex(peer.as_bytes()),
    });
    if let Some(object) = value.as_object_mut() {
        for (field, granted) in GRANT_FIELDS.into_iter().zip(grants) {
            object.insert(field.to_owned(), Value::Bool(granted));
        }
    }
    value
}

pub(super) fn exposure_value(
    operation: &str,
    (id, space, expose): (RequestId, ma2a_core::SpaceId, bool),
    spec: &ExposureSpec,
) -> Value {
    let mut value = json!({
        "operation": operation,
        "request_id": encode_hex(id.as_bytes()),
        "space_id": encode_hex(space.as_bytes()),
    });
    let fields = spec_fields(spec);
    if let Some(object) = value.as_object_mut() {
        for (name, field) in fields {
            if expose || !matches!(name, "path" | "access") {
                object.insert(name.to_owned(), field);
            }
        }
    }
    value
}

/// Returns the flat nullable wire fields of one exposure.
pub(crate) fn spec_fields(spec: &ExposureSpec) -> [(&'static str, Value); 7] {
    let (kind, root, local, proxy) = match spec {
        ExposureSpec::FilesRoot { root, local } => ("files_root", Some(root), local.as_ref(), None),
        ExposureSpec::Exec => ("exec", None, None, None),
        ExposureSpec::Shell => ("shell", None, None, None),
        ExposureSpec::Process => ("process", None, None, None),
        ExposureSpec::Proxy(rule) => ("proxy", None, None, Some(rule)),
    };
    [
        ("kind", json!(kind)),
        ("root", json!(root.map(RootName::as_str))),
        ("path", json!(local.map(|(path, _)| path.as_str()))),
        ("access", json!(local.map(|(_, access)| access.name()))),
        ("host", json!(proxy.map(|rule| proxy_host(rule.host())))),
        ("port_start", json!(proxy.map(|rule| rule.ports().0))),
        ("port_end", json!(proxy.map(|rule| rule.ports().1))),
    ]
}

/// Returns the wire form of a proxy host pattern: `*` or the canonical host.
pub(crate) fn proxy_host(host: &ProxyHostPattern) -> &str {
    match host {
        ProxyHostPattern::Any => "*",
        ProxyHostPattern::Exact(host) => host.as_str(),
    }
}
