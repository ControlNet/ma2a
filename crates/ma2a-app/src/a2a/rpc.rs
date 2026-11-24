//! The A2A v1.0 JSON-RPC 2.0 envelope: requests, results, and errors.

use axum::http::HeaderMap;
use serde_json::{Value, json};

use super::backend::{Refusal, Route};

/// The only A2A protocol version this adapter speaks.
pub(super) const VERSION: &str = "1.0";

/// One JSON-RPC error with its A2A `ErrorInfo` reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RpcError {
    pub(super) code: i64,
    pub(super) message: String,
    pub(super) reason: String,
}

impl RpcError {
    pub(super) fn new(code: i64, message: &str, reason: &str) -> Self {
        Self {
            code,
            message: message.to_owned(),
            reason: reason.to_owned(),
        }
    }

    pub(super) fn parse() -> Self {
        Self::new(-32_700, "Invalid JSON payload", "PARSE_ERROR")
    }

    pub(super) fn invalid_request() -> Self {
        Self::new(
            -32_600,
            "Request payload validation error",
            "INVALID_REQUEST",
        )
    }

    pub(super) fn invalid_params(why: &str) -> Self {
        Self::new(-32_602, why, "INVALID_PARAMS")
    }

    pub(super) fn method_not_found() -> Self {
        Self::new(-32_601, "Method not found", "METHOD_NOT_FOUND")
    }

    pub(super) fn task_not_found() -> Self {
        Self::new(-32_001, "Task not found", "TASK_NOT_FOUND")
    }

    pub(super) fn not_cancelable() -> Self {
        Self::new(-32_002, "Task cannot be canceled", "TASK_NOT_CANCELABLE")
    }

    pub(super) fn push_not_supported() -> Self {
        Self::new(
            -32_003,
            "Push notifications are not supported",
            "PUSH_NOTIFICATION_NOT_SUPPORTED",
        )
    }

    pub(super) fn unsupported(why: &str) -> Self {
        Self::new(-32_004, why, "UNSUPPORTED_OPERATION")
    }

    pub(super) fn content_type(why: &str) -> Self {
        Self::new(-32_005, why, "CONTENT_TYPE_NOT_SUPPORTED")
    }

    pub(super) fn no_extended_card() -> Self {
        Self::new(
            -32_007,
            "No extended Agent Card is configured",
            "EXTENDED_AGENT_CARD_NOT_CONFIGURED",
        )
    }

    pub(super) fn version() -> Self {
        Self::new(
            -32_009,
            "Only A2A-Version 1.0 is supported",
            "VERSION_NOT_SUPPORTED",
        )
    }

    /// Maps a refusal of `method` onto the A2A error a client acts on.
    pub(super) fn refused(method: &str, refusal: &Refusal) -> Self {
        let name = refusal.0.as_str();
        let on_task = method != "SendMessage" && method != "SendStreamingMessage";
        match name {
            "not_found" | "unknown_task" if on_task || name == "unknown_task" => {
                Self::task_not_found()
            }
            "not_found" | "unknown_agent" => Self::new(-32_603, "Agent not found", "UNKNOWN_AGENT"),
            "task_ended" | "conflict" if method == "CancelTask" => Self::not_cancelable(),
            "task_ended" => Self::unsupported("the task has already ended"),
            "invalid_input" => Self::invalid_params("the Runtime refused the parameters"),
            other => Self::new(
                -32_603,
                &format!("the Runtime refused the operation: {other}"),
                &other.to_ascii_uppercase(),
            ),
        }
    }

    /// The JSON-RPC response carrying this error for request `id`.
    pub(super) fn response(&self, id: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": self.code,
                "message": self.message,
                "data": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": self.reason,
                    "domain": "a2a-protocol.org",
                }],
            },
        })
    }
}

/// The JSON-RPC response carrying `result` for request `id`.
pub(super) fn success(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// One parsed JSON-RPC request.
#[derive(Debug)]
pub(super) struct Request {
    pub(super) id: Value,
    pub(super) method: String,
    pub(super) params: Value,
}

/// Parses one JSON-RPC request; a failure carries the `id` to answer, if it was readable.
pub(super) fn parse(body: &[u8]) -> Result<Request, (Value, RpcError)> {
    let document: Value =
        serde_json::from_slice(body).map_err(|_| (Value::Null, RpcError::parse()))?;
    let id = document.get("id").cloned().unwrap_or(Value::Null);
    let valid_id = matches!(id, Value::String(_) | Value::Number(_));
    match (
        document.get("jsonrpc").and_then(Value::as_str),
        document.get("method").and_then(Value::as_str),
    ) {
        (Some("2.0"), Some(method)) if valid_id => Ok(Request {
            method: method.to_owned(),
            params: document.get("params").cloned().unwrap_or_else(|| json!({})),
            id,
        }),
        _ => Err((id, RpcError::invalid_request())),
    }
}

/// Checks the `A2A-Version` service parameter, from the header or the query.
pub(super) fn version(headers: &HeaderMap, query: Option<&str>) -> Result<(), RpcError> {
    let from_query = query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|pair| {
            pair.split_once('=')
                .filter(|(name, _)| name.eq_ignore_ascii_case("A2A-Version"))
                .map(|(_, value)| value.to_owned())
        });
    let sent = headers
        .get("A2A-Version")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or(from_query);
    // Without the parameter a client means 0.3, which this adapter does not speak.
    match sent.as_deref().map(str::trim) {
        Some(VERSION) => Ok(()),
        _ => Err(RpcError::version()),
    }
}

/// The `RequestId` of the operation one A2A message causes on `route`.
///
/// A client's `messageId` is unique per message, so a resend of the same message replays
/// and a different message under the same ID conflicts.
pub(super) fn request_id(route: &Route, message_id: &str) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("ma2a a2a-adapter v1 message request id");
    hasher.update(route.path().as_bytes());
    hasher.update(&[0]);
    hasher.update(message_id.as_bytes());
    hasher
        .finalize()
        .to_hex()
        .get(..32)
        .unwrap_or_default()
        .to_owned()
}

/// The string parameter `name`, which must be present and non-empty.
pub(super) fn required<'a>(params: &'a Value, name: &str) -> Result<&'a str, RpcError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::invalid_params(&format!("`{name}` is required")))
}

/// The optional string parameter `name`. Absent, null, or empty names nothing, and any
/// other value than a string is refused rather than read as absent.
pub(super) fn optional<'a>(params: &'a Value, name: &str) -> Result<Option<&'a str>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str()).filter(|value| !value.is_empty())),
        Some(_) => Err(RpcError::invalid_params(&format!("`{name}` is a string"))),
    }
}

/// The optional non-negative integer parameter `name`.
pub(super) fn count(params: &Value, name: &str) -> Result<Option<usize>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .map(Some)
            .ok_or_else(|| RpcError::invalid_params(&format!("`{name}` must be non-negative"))),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use ma2a_core::service::AgentId;
    use serde_json::json;

    use super::{Refusal, Route, RpcError, parse, request_id, version};

    #[test]
    fn requests_need_the_2_0_envelope_and_an_id() {
        let parsed = parse(br#"{"jsonrpc":"2.0","id":7,"method":"GetTask","params":{"id":"x"}}"#);
        assert!(parsed.is_ok_and(|request| request.method == "GetTask"));
        assert_eq!(parse(b"{").err(), Some((json!(null), RpcError::parse())));
        let no_id = parse(br#"{"jsonrpc":"2.0","method":"GetTask"}"#);
        assert_eq!(
            no_id.err(),
            Some((json!(null), RpcError::invalid_request()))
        );
    }

    #[test]
    fn only_version_1_0_is_spoken() {
        let mut headers = HeaderMap::new();
        assert_eq!(version(&headers, None), Err(RpcError::version()));
        assert_eq!(version(&headers, Some("A2A-Version=1.0")), Ok(()));
        headers.insert("A2A-Version", HeaderValue::from_static("0.3"));
        assert_eq!(version(&headers, None), Err(RpcError::version()));
        headers.insert("A2A-Version", HeaderValue::from_static("1.0"));
        assert_eq!(version(&headers, None), Ok(()));
    }

    #[test]
    fn a_message_id_names_one_request_per_route() -> Result<(), Box<dyn std::error::Error>> {
        let worker = Route::Local(AgentId::new("worker")?);
        let lead = Route::Local(AgentId::new("lead")?);
        assert_eq!(request_id(&worker, "m-1"), request_id(&worker, "m-1"));
        assert_ne!(request_id(&worker, "m-1"), request_id(&worker, "m-2"));
        assert_ne!(request_id(&worker, "m-1"), request_id(&lead, "m-1"));
        assert_eq!(request_id(&worker, "m-1").len(), 32);
        Ok(())
    }

    #[test]
    fn refusals_map_onto_a2a_errors() {
        let refused =
            |method: &str, name: &str| RpcError::refused(method, &Refusal::named(name)).code;
        assert_eq!(refused("GetTask", "not_found"), -32_001);
        assert_eq!(refused("SendMessage", "unknown_task"), -32_001);
        assert_eq!(refused("CancelTask", "task_ended"), -32_002);
        assert_eq!(refused("SendMessage", "task_ended"), -32_004);
        assert_eq!(refused("SendMessage", "not_found"), -32_603);
    }
}
