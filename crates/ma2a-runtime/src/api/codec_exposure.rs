//! Decoding of one exposure selector in its flat nullable wire form.

use ma2a_core::service::{
    AgentId, ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName, ServiceName,
};
use serde_json::{Map, Value};

use super::{
    ApiError, codec_fields::nullable, codec_services::ExposureSpec, commands::BoundedText,
};

/// Most bytes of an exposed root's local directory path.
const MAX_ROOT_PATH_BYTES: usize = 4_096;

pub(super) fn exposure_spec(
    object: &Map<String, Value>,
    expose: bool,
) -> Result<ExposureSpec, ApiError> {
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(ApiError::invalid_input)?;
    let root = nullable(object, "root")?;
    let agent = nullable(object, "agent")?;
    let service = nullable(object, "service")?;
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
    let named = agent.is_none() && service.is_none();
    let root_only = host.is_none() && ports == (None, None) && named;
    let selectors = root.is_none() && path.is_none() && access.is_none();
    let empty = selectors && root_only;
    let unrouted = selectors && host.is_none() && ports == (None, None);
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
        "agent" if unrouted && service.is_none() => {
            let agent =
                AgentId::new(agent.ok_or_else(ApiError::invalid_input)?).map_err(ApiError::new)?;
            Ok(ExposureSpec::Agent(agent))
        }
        "service" if unrouted && agent.is_none() => {
            let service = ServiceName::new(service.ok_or_else(ApiError::invalid_input)?)
                .map_err(ApiError::new)?;
            Ok(ExposureSpec::Service(service))
        }
        "proxy" if selectors && named => {
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
