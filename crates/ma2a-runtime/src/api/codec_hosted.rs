//! Local API commands for the services this Runtime hosts, and the wire form of a manifest.

use ma2a_core::{
    RequestId,
    service::{Interaction, OperationName, ServiceManifest, ServiceName, ServiceVersion},
};
use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_fields::{encode_hex, exact_fields, fixed_hex, nullable, number, request_id, text},
    commands::CommandKind,
};

/// One local command on hosted services.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostedCommand {
    /// Register a new service, stopped, or replace a stopped one.
    Register(RequestId, ServiceName, Box<ServiceManifest>),
    /// Remove one stopped service's registration.
    Unregister(RequestId, ServiceName),
    /// Start one service.
    Start(RequestId, ServiceName),
    /// Stop one service.
    Stop(RequestId, ServiceName),
    /// Every registration after this one.
    List(Option<ServiceName>),
    /// Upgrade the connection into the service's link.
    Attach(ServiceName),
    /// Upgrade the connection into one call of the service.
    Accept(ServiceName, [u8; 16]),
}

impl HostedCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Register(..) => "service_register",
            Self::Unregister(..) => "service_unregister",
            Self::Start(..) => "service_start",
            Self::Stop(..) => "service_stop",
            Self::List(_) => "service_hosted_list",
            Self::Attach(_) => "service_attach",
            Self::Accept(..) => "service_accept",
        }
    }

    pub(crate) const fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::Register(id, ..)
            | Self::Unregister(id, _)
            | Self::Start(id, _)
            | Self::Stop(id, _) => Some(*id),
            Self::List(_) | Self::Attach(_) | Self::Accept(..) => None,
        }
    }

    pub(super) fn to_value(&self) -> Value {
        let operation = self.operation();
        let request = |id: &RequestId| encode_hex(id.as_bytes());
        match self {
            Self::Register(id, service, manifest) => json!({
                "operation": operation,
                "request_id": request(id),
                "service": service.as_str(),
                "manifest": manifest_value(manifest),
            }),
            Self::Unregister(id, service) | Self::Start(id, service) | Self::Stop(id, service) => {
                json!({"operation": operation, "request_id": request(id), "service": service.as_str()})
            }
            Self::List(after) => {
                json!({"operation": operation, "after": after.as_ref().map(ServiceName::as_str)})
            }
            Self::Attach(service) => json!({"operation": operation, "service": service.as_str()}),
            Self::Accept(service, call) => json!({
                "operation": operation,
                "service": service.as_str(),
                "call_id": encode_hex(call),
            }),
        }
    }
}

/// Returns the wire form of one manifest.
pub(crate) fn manifest_value(manifest: &ServiceManifest) -> Value {
    let versions: Vec<Value> = manifest
        .versions()
        .iter()
        .map(|version| {
            let operations: Vec<Value> = version
                .operations()
                .iter()
                .map(|(name, interaction)| {
                    json!({"name": name.as_str(), "interaction": interaction.name()})
                })
                .collect();
            json!({"version": version.version(), "operations": operations})
        })
        .collect();
    json!({
        "description": manifest.description(),
        "max_calls": manifest.max_calls(),
        "versions": versions,
    })
}

fn array<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a [Value], ApiError> {
    object
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(ApiError::invalid_input)
}

fn member(value: &Value) -> Result<&Map<String, Value>, ApiError> {
    value.as_object().ok_or_else(ApiError::invalid_input)
}

/// Parses the wire form of one manifest; every bound is the manifest's own.
fn parse_manifest(value: Option<&Value>) -> Result<ServiceManifest, ApiError> {
    let object = member(value.ok_or_else(ApiError::invalid_input)?)?;
    exact_fields(object, &["description", "max_calls", "versions"])?;
    let versions = array(object, "versions")?
        .iter()
        .map(|version| {
            let version = member(version)?;
            exact_fields(version, &["version", "operations"])?;
            let operations = array(version, "operations")?
                .iter()
                .map(|operation| {
                    let operation = member(operation)?;
                    exact_fields(operation, &["name", "interaction"])?;
                    let name = OperationName::new(text(operation, "name")?)
                        .map_err(|_| ApiError::invalid_input())?;
                    let interaction = Interaction::from_name(text(operation, "interaction")?)
                        .ok_or_else(ApiError::invalid_input)?;
                    Ok((name, interaction))
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            let number = u16::try_from(number(version, "version")?)
                .map_err(|_| ApiError::invalid_input())?;
            ServiceVersion::new(number, operations).map_err(|_| ApiError::invalid_input())
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let max_calls =
        u8::try_from(number(object, "max_calls")?).map_err(|_| ApiError::invalid_input())?;
    ServiceManifest::new(text(object, "description")?, versions, max_calls)
        .map_err(|_| ApiError::invalid_input())
}

fn service(object: &Map<String, Value>) -> Result<ServiceName, ApiError> {
    ServiceName::new(text(object, "service")?).map_err(|_| ApiError::invalid_input())
}

/// Whether `operation` is one of the hosted-service commands.
pub(super) fn is_hosted(operation: &str) -> bool {
    matches!(
        operation,
        "service_register"
            | "service_unregister"
            | "service_start"
            | "service_stop"
            | "service_hosted_list"
            | "service_attach"
            | "service_accept"
    )
}

/// Parses one hosted-service command.
pub(super) fn parse(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    let named = ["version", "operation", "request_id", "service"];
    let command = match text(object, "operation")? {
        "service_register" => {
            exact_fields(
                object,
                &["version", "operation", "request_id", "service", "manifest"],
            )?;
            let manifest = parse_manifest(object.get("manifest"))?;
            HostedCommand::Register(
                request_id(object, "request_id")?,
                service(object)?,
                Box::new(manifest),
            )
        }
        operation @ ("service_unregister" | "service_start" | "service_stop") => {
            exact_fields(object, &named)?;
            let (id, name) = (request_id(object, "request_id")?, service(object)?);
            match operation {
                "service_unregister" => HostedCommand::Unregister(id, name),
                "service_start" => HostedCommand::Start(id, name),
                _ => HostedCommand::Stop(id, name),
            }
        }
        "service_hosted_list" => {
            exact_fields(object, &["version", "operation", "after"])?;
            let after = nullable(object, "after")?
                .map(|after| ServiceName::new(after).map_err(|_| ApiError::invalid_input()))
                .transpose()?;
            HostedCommand::List(after)
        }
        "service_attach" => {
            exact_fields(object, &["version", "operation", "service"])?;
            HostedCommand::Attach(service(object)?)
        }
        "service_accept" => {
            exact_fields(object, &["version", "operation", "service", "call_id"])?;
            HostedCommand::Accept(service(object)?, fixed_hex(object, "call_id")?)
        }
        _ => return Err(ApiError::invalid_input()),
    };
    Ok(CommandKind::Hosted(command))
}
