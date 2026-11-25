//! The JSON-RPC envelope, the A2A version parameter, refusals, and request IDs.

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
    // An id that is neither a string nor a number is answered with a null id, not echoed.
    for id in ["[1]", "{}", "true"] {
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"GetTask"}}"#);
        let invalid = parse(body.as_bytes()).err();
        assert_eq!(
            invalid,
            Some((json!(null), RpcError::invalid_request())),
            "{id}"
        );
    }
    // Parameters are named, so they are an object, or absent.
    let named = |params: &str| {
        let body = format!(r#"{{"jsonrpc":"2.0","id":7,"method":"ListTasks"{params}}}"#);
        parse(body.as_bytes()).map(|request| request.params)
    };
    assert_eq!(named(""), Ok(json!({})));
    assert_eq!(named(r#","params":null"#), Ok(json!({})));
    for params in [r#","params":[1,2]"#, r#","params":"x""#, r#","params":7"#] {
        assert_eq!(
            named(params).err(),
            Some((json!(7), RpcError::invalid_request())),
            "{params}"
        );
    }
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
    let refused = |method: &str, name: &str| RpcError::refused(method, &Refusal::named(name)).code;
    assert_eq!(refused("GetTask", "not_found"), -32_001);
    assert_eq!(refused("SendMessage", "unknown_task"), -32_001);
    assert_eq!(refused("CancelTask", "task_ended"), -32_002);
    assert_eq!(refused("SendMessage", "task_ended"), -32_004);
    assert_eq!(refused("SendMessage", "not_found"), -32_603);
}
