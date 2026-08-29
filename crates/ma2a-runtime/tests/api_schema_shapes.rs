//! Recursive machine-schema checks for every Rust local API wire value.

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/schema_types.rs"]
mod schema_types;
#[path = "support/schema_validator.rs"]
mod schema_validator;

use ma2a_core::{ProtocolError, RequestId};
use ma2a_runtime::api::{
    Command, CommandResult, ERROR_NAMES, LOCAL_API_SCHEMA_JSON, RESULT_NAMES, ReplayDecision,
    RuntimeApiBoundary, dispatch_request, encode_command, encode_error, encode_event,
    encode_response,
};
use schema_validator::SchemaValidator;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
struct ShapeBoundary {
    result: Result<CommandResult, ProtocolError>,
}

impl RuntimeApiBoundary for ShapeBoundary {
    fn state_revision(&mut self) -> u64 {
        7
    }

    fn replay_decision(&mut self, _: RequestId, _: [u8; 32]) -> ReplayDecision {
        ReplayDecision::FRESH
    }

    fn replay_result(&mut self, _: RequestId) -> Result<CommandResult, ProtocolError> {
        self.result.clone()
    }

    fn execute(&mut self, _: &Command) -> Result<CommandResult, ProtocolError> {
        self.result.clone()
    }
}

#[test]
fn every_command_serialization_recursively_matches_the_machine_schema() -> TestResult {
    let validator = SchemaValidator::new(LOCAL_API_SCHEMA_JSON)?;
    let mut operations = Vec::new();
    for command in fixtures::commands()? {
        let encoded = encode_command(&command)?;
        let value: Value = serde_json::from_slice(&encoded)?;
        validator.command(&value)?;
        operations.push(command.operation());
    }
    assert_eq!(operations, ma2a_runtime::api::COMMAND_NAMES);
    Ok(())
}

#[test]
fn every_result_and_success_envelope_recursively_matches_the_machine_schema() -> TestResult {
    let validator = SchemaValidator::new(LOCAL_API_SCHEMA_JSON)?;
    let mut result_types = Vec::new();
    for result in fixtures::results()? {
        result_types.push(result.result_type());
        let mut boundary = ShapeBoundary { result: Ok(result) };
        let response =
            dispatch_request(br#"{"version":1,"operation":"handshake"}"#, &mut boundary)?;
        let encoded = encode_response(&response)?;
        let value: Value = serde_json::from_slice(&encoded)?;
        validator.response("success", &value)?;
    }
    assert_eq!(result_types, RESULT_NAMES);
    Ok(())
}

#[test]
fn every_error_envelope_recursively_matches_the_machine_schema() -> TestResult {
    let validator = SchemaValidator::new(LOCAL_API_SCHEMA_JSON)?;
    let inputs = [
        (
            br#"{"version":2,"operation":"handshake"}"#.as_slice(),
            ProtocolError::INTERNAL,
        ),
        (
            br#"{"version":1,"operation":"unknown"}"#.as_slice(),
            ProtocolError::INTERNAL,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::UNAUTHORIZED,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::NOT_FOUND,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::CONFLICT,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::EXPIRED,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::ROLLBACK,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::UNAVAILABLE,
        ),
        (
            br#"{"version":1,"operation":"handshake"}"#.as_slice(),
            ProtocolError::INTERNAL,
        ),
    ];
    for ((input, boundary_error), expected) in inputs.into_iter().zip(ERROR_NAMES) {
        let error = dispatch_error(input, boundary_error)?;
        assert_eq!(error.code().name(), expected);
        let encoded = encode_error(error)?;
        let value: Value = serde_json::from_slice(&encoded)?;
        validator.response("error", &value)?;
    }
    Ok(())
}

#[test]
fn every_event_serialization_recursively_matches_the_machine_schema() -> TestResult {
    let validator = SchemaValidator::new(LOCAL_API_SCHEMA_JSON)?;
    for event in fixtures::events()? {
        let encoded = encode_event(&event)?;
        let value: Value = serde_json::from_slice(&encoded)?;
        validator.event(&value)?;
    }
    Ok(())
}

fn dispatch_error(
    input: &[u8],
    boundary_error: ProtocolError,
) -> Result<ma2a_runtime::api::ApiError, std::io::Error> {
    let mut boundary = ShapeBoundary {
        result: Err(boundary_error),
    };
    match dispatch_request(input, &mut boundary) {
        Err(error) => Ok(error),
        Ok(_) => Err(std::io::Error::other("fixture request must fail")),
    }
}
