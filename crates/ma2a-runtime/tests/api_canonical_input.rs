//! Canonical local API input rejection and ordering regressions.

use ma2a_core::{ProtocolError, RequestId};
use ma2a_runtime::api::{
    Command, CommandResult, ReplayDecision, RuntimeApiBoundary, decode_command, dispatch_request,
};

const REQUEST_ID: &str = "000102030405060708090a0b0c0d0e0f";
const SPACE_ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Debug, Default)]
struct CallbackProbe {
    callbacks: usize,
}

impl RuntimeApiBoundary for CallbackProbe {
    fn state_revision(&mut self) -> u64 {
        self.callbacks += 1;
        0
    }

    fn replay_decision(&mut self, _: RequestId, _: [u8; 32]) -> ReplayDecision {
        self.callbacks += 1;
        ReplayDecision::FRESH
    }

    fn replay_result(&mut self, _: RequestId) -> Result<CommandResult, ProtocolError> {
        self.callbacks += 1;
        Ok(CommandResult::shutting_down())
    }

    fn execute(&mut self, _: &Command) -> Result<CommandResult, ProtocolError> {
        self.callbacks += 1;
        Ok(CommandResult::shutting_down())
    }
}

#[test]
fn rejects_duplicate_root_members_without_callbacks() {
    let inputs = [
        br#"{"version":1,"version":1,"operation":"handshake"}"#.as_slice(),
        br#"{"version":1,"operation":"handshake","operation":"handshake"}"#.as_slice(),
    ];

    for input in inputs {
        let mut boundary = CallbackProbe::default();
        let error = dispatch_request(input, &mut boundary).expect_err("duplicates must fail");

        assert_eq!(error.code(), ProtocolError::INVALID_INPUT);
        assert_eq!(boundary.callbacks, 0);
    }
}

#[test]
fn preserves_unambiguous_version_rejection_before_other_duplicate_members() {
    let input = br#"{"version":2,"operation":"handshake","operation":"handshake"}"#;
    let mut boundary = CallbackProbe::default();

    let error = dispatch_request(input, &mut boundary).expect_err("version 2 must fail first");

    assert_eq!(error.code(), ProtocolError::VERSION_MISMATCH);
    assert_eq!(boundary.callbacks, 0);
}

#[test]
fn rejects_noncanonical_uppercase_identifiers() {
    let inputs = [
        format!(
            r#"{{"version":1,"operation":"space_create","request_id":"{}","name":"ops"}}"#,
            REQUEST_ID.to_uppercase()
        ),
        format!(
            r#"{{"version":1,"operation":"space_show","space_id":"{}"}}"#,
            SPACE_ID.to_uppercase()
        ),
        format!(
            r#"{{"version":1,"operation":"control_sync_status","peer_endpoint_id":"{}"}}"#,
            "aa".repeat(32).to_uppercase()
        ),
    ];

    for input in inputs {
        let error = decode_command(input.as_bytes()).expect_err("uppercase IDs must fail");
        assert_eq!(error.code(), ProtocolError::INVALID_INPUT);
    }
}
