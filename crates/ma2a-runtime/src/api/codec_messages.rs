//! The wire form of Agent messages: `{role, parts: [{kind, content}]}`, `data` parts
//! carrying JSON text.

use ma2a_core::service::{AgentMessage, MessagePart, MessageRole};
use serde_json::{Value, json};

use super::{
    ApiError,
    codec_fields::{exact_fields, text},
};

/// Encodes one message as `{role, parts: [{kind, content}]}`.
pub(super) fn message_value(message: &AgentMessage) -> Value {
    let parts: Vec<Value> = message
        .parts()
        .iter()
        .map(|part| {
            let kind = match part {
                MessagePart::Text(_) => "text",
                MessagePart::Data(_) => "data",
            };
            json!({"kind": kind, "content": part.content()})
        })
        .collect();
    json!({"role": message.role().name(), "parts": parts})
}

pub(super) fn parse_message(value: Option<&Value>) -> Result<AgentMessage, ApiError> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(ApiError::invalid_input)?;
    exact_fields(object, &["role", "parts"])?;
    let role = MessageRole::from_name(text(object, "role")?).ok_or_else(ApiError::invalid_input)?;
    let parts = object
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(ApiError::invalid_input)?
        .iter()
        .map(|part| {
            let part = part.as_object().ok_or_else(ApiError::invalid_input)?;
            exact_fields(part, &["kind", "content"])?;
            let content = text(part, "content")?.to_owned();
            match text(part, "kind")? {
                "text" => Ok(MessagePart::Text(content)),
                "data" => Ok(MessagePart::Data(content)),
                _ => Err(ApiError::invalid_input()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    AgentMessage::new(role, parts).map_err(|_| ApiError::invalid_input())
}
