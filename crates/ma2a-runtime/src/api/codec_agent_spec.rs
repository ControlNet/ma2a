//! The wire form of one Agent definition in `agent_define`.

use ma2a_agent::tools::ToolFamily;
use ma2a_core::{RequestId, service::AgentId};
use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_fields::{encode_hex, exact_fields, nullable, number, text},
};
use crate::agents::AgentSpec;

pub(super) fn spec_value(operation: &str, id: RequestId, spec: &AgentSpec) -> Value {
    let env: Vec<Value> = spec
        .env
        .iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect();
    let tools: Vec<&str> = ToolFamily::of_set(spec.tools)
        .into_iter()
        .map(ToolFamily::name)
        .collect();
    json!({
        "operation": operation,
        "request_id": encode_hex(id.as_bytes()),
        "agent": spec.id.as_str(),
        "description": spec.description,
        "program": spec.command.0,
        "args": spec.command.1,
        "cwd": spec.cwd,
        "env": env,
        "tools": tools,
        "max_tasks": spec.max_tasks,
    })
}

fn strings(value: Option<&Value>) -> Result<Vec<String>, ApiError> {
    value
        .and_then(Value::as_array)
        .ok_or_else(ApiError::invalid_input)?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(ApiError::invalid_input)
        })
        .collect()
}

pub(super) fn parse_spec(object: &Map<String, Value>) -> Result<AgentSpec, ApiError> {
    let env = object
        .get("env")
        .and_then(Value::as_array)
        .ok_or_else(ApiError::invalid_input)?
        .iter()
        .map(|pair| {
            let pair = pair.as_object().ok_or_else(ApiError::invalid_input)?;
            exact_fields(pair, &["name", "value"])?;
            Ok((
                text(pair, "name")?.to_owned(),
                text(pair, "value")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let mut tools = 0;
    for name in strings(object.get("tools"))? {
        tools |= ToolFamily::from_name(&name)
            .ok_or_else(ApiError::invalid_input)?
            .bit();
    }
    Ok(AgentSpec {
        id: AgentId::new(text(object, "agent")?).map_err(|_| ApiError::invalid_input())?,
        description: text(object, "description")?.to_owned(),
        command: (
            text(object, "program")?.to_owned(),
            strings(object.get("args"))?,
        ),
        cwd: nullable(object, "cwd")?.map(str::to_owned),
        env,
        tools,
        max_tasks: u8::try_from(number(object, "max_tasks")?)
            .map_err(|_| ApiError::invalid_input())?,
    })
}
