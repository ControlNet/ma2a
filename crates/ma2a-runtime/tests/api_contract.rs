//! Adversarial coverage for the exact local Runtime API v1 boundary.

use std::fmt::Write;

use ma2a_core::{ProtocolError, RequestId};
use ma2a_runtime::api::{
    COMMAND_NAMES, ERROR_NAMES, EVENT_NAMES, LOCAL_API_SCHEMA_JSON, LOCAL_API_SCHEMA_SHA256,
    LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES, RESULT_NAMES, ReplayDecision, RuntimeApiBoundary,
    classify_event_revision, decode_command, dispatch_request, encode_command,
};
use sha2::{Digest, Sha256};

const REQUEST_ID: &str = "000102030405060708090a0b0c0d0e0f";
const ENDPOINT_ID: &str = "5866666666666666666666666666666666666666666666666666666666666666";
const SPACE_ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Debug, Default)]
struct BoundaryProbe {
    state_reads: usize,
    replay_reads: usize,
    replay_results: usize,
    mutations: usize,
    replay: ReplayDecision,
}

impl RuntimeApiBoundary for BoundaryProbe {
    fn state_revision(&mut self) -> u64 {
        self.state_reads += 1;
        7
    }

    fn replay_decision(
        &mut self,
        _request_id: RequestId,
        _fingerprint: [u8; 32],
    ) -> ReplayDecision {
        self.replay_reads += 1;
        self.replay
    }

    fn execute(
        &mut self,
        _command: &ma2a_runtime::api::Command,
    ) -> Result<ma2a_runtime::api::CommandResult, ProtocolError> {
        self.mutations += 1;
        Ok(ma2a_runtime::api::CommandResult::shutting_down())
    }

    fn replay_result(
        &mut self,
        _request_id: RequestId,
    ) -> Result<ma2a_runtime::api::CommandResult, ProtocolError> {
        self.replay_results += 1;
        Ok(ma2a_runtime::api::CommandResult::shutting_down())
    }
}

#[test]
fn round_trips_every_command_variant_through_the_transport_neutral_codec() {
    let requests = [
        r#"{"version":1,"operation":"handshake"}"#.to_owned(),
        r#"{"version":1,"operation":"status"}"#.to_owned(),
        r#"{"version":1,"operation":"endpoint_info"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"space_create","request_id":"{REQUEST_ID}","name":"ops"}}"#
        ),
        r#"{"version":1,"operation":"space_list"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"space_show","space_id":"{SPACE_ID}"}}"#),
        format!(
            r#"{{"version":1,"operation":"space_invite","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"space_redeem","request_id":"{REQUEST_ID}","invitation":"ticket"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"space_revoke","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"control_sync_status","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"{REQUEST_ID}","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"private_relay_configure","request_id":"{REQUEST_ID}","mode":"native_tls","host":"relay.example","port":443}}"#
        ),
        r#"{"version":1,"operation":"private_relay_status"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"public_relay_configure","request_id":"{REQUEST_ID}","url":"https://relay.example"}}"#
        ),
        r#"{"version":1,"operation":"public_relay_status"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"echo_call","request_id":"{REQUEST_ID}","target_endpoint_id":"{ENDPOINT_ID}","payload":"hello"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"ui_password_set","request_id":"{REQUEST_ID}","password":"correct horse battery staple"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"ui_password_reset","request_id":"{REQUEST_ID}","password":"new correct horse battery staple"}}"#
        ),
        format!(r#"{{"version":1,"operation":"session_revoke_all","request_id":"{REQUEST_ID}"}}"#),
        r#"{"version":1,"operation":"snapshot_fetch"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"graceful_shutdown","request_id":"{REQUEST_ID}"}}"#),
    ];

    let operations = requests
        .iter()
        .map(|request| {
            let command = decode_command(request.as_bytes()).expect("fixture command must decode");
            let encoded = encode_command(&command).expect("typed command must encode");
            let decoded = decode_command(&encoded).expect("encoded command must decode");
            assert_eq!(decoded, command);
            command.operation()
        })
        .collect::<Vec<_>>();

    assert_eq!(operations, COMMAND_NAMES);
}

#[test]
fn replays_same_request_without_executing_mutation() {
    let input = format!(
        "{{\"version\":1,\"operation\":\"ui_password_set\",\"request_id\":\"{REQUEST_ID}\",\"password\":\"correct horse battery staple\"}}"
    );
    let mut boundary = BoundaryProbe {
        replay: ReplayDecision::REPLAY,
        ..BoundaryProbe::default()
    };

    let response = dispatch_request(input.as_bytes(), &mut boundary).expect("replay must succeed");

    assert_eq!(response.revision(), 7);
    assert_eq!(boundary.replay_reads, 1);
    assert_eq!(boundary.replay_results, 1);
    assert_eq!(boundary.state_reads, 1);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn rejects_version_two_before_any_boundary_callback() {
    // Given
    let input = br#"{"version":2,"operation":"unknown","unexpected":true}"#;
    let mut boundary = BoundaryProbe::default();

    // When
    let error = dispatch_request(input, &mut boundary).expect_err("version 2 must fail");

    // Then
    assert_eq!(error.code(), ProtocolError::VERSION_MISMATCH);
    assert_eq!(boundary.state_reads, 0);
    assert_eq!(boundary.replay_reads, 0);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn transport_adapter_can_preflight_version_without_runtime_state() {
    // Given
    let input = br#"{"version":9,"operation":"status"}"#;
    let mut boundary = BoundaryProbe::default();

    // When
    let error = dispatch_request(input, &mut boundary).expect_err("version 9 must fail");

    // Then
    assert_eq!(error.code(), ProtocolError::VERSION_MISMATCH);
    assert_eq!(boundary.state_reads, 0);
    assert_eq!(boundary.replay_reads, 0);
    assert_eq!(boundary.replay_results, 0);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn rejects_unknown_operation_without_revision_change() {
    let input = br#"{"version":1,"operation":"future_operation"}"#;
    let mut boundary = BoundaryProbe::default();

    let error = dispatch_request(input, &mut boundary).expect_err("unknown operation must fail");

    assert_eq!(error.code(), ProtocolError::INVALID_INPUT);
    assert_eq!(boundary.state_reads, 0);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn rejects_conflicting_request_id_before_mutation_or_revision_read() {
    let input = format!(
        "{{\"version\":1,\"operation\":\"ui_password_set\",\"request_id\":\"{REQUEST_ID}\",\"password\":\"correct horse battery staple\"}}"
    );
    let mut boundary = BoundaryProbe {
        replay: ReplayDecision::CONFLICT,
        ..BoundaryProbe::default()
    };

    let error = dispatch_request(input.as_bytes(), &mut boundary).expect_err("conflict must fail");

    assert_eq!(error.code(), ProtocolError::CONFLICT);
    assert_eq!(boundary.replay_reads, 1);
    assert_eq!(boundary.state_reads, 0);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn rejects_oversize_input_before_parsing_or_callbacks() {
    let input = vec![b' '; MAX_LOCAL_REQUEST_BYTES + 1];
    let mut boundary = BoundaryProbe::default();

    let error = dispatch_request(&input, &mut boundary).expect_err("oversize input must fail");

    assert_eq!(error.code(), ProtocolError::INVALID_INPUT);
    assert_eq!(boundary.state_reads, 0);
    assert_eq!(boundary.replay_reads, 0);
    assert_eq!(boundary.mutations, 0);
}

#[test]
fn freezes_version_variants_gap_policy_and_schema_hash() {
    assert_eq!(LOCAL_API_VERSION, 1);
    assert_eq!(COMMAND_NAMES.len(), 21);
    assert_eq!(RESULT_NAMES.len(), 21);
    assert_eq!(ERROR_NAMES.len(), 9);
    assert_eq!(
        ERROR_NAMES,
        [
            ProtocolError::VERSION_MISMATCH.name(),
            ProtocolError::INVALID_INPUT.name(),
            ProtocolError::UNAUTHORIZED.name(),
            ProtocolError::NOT_FOUND.name(),
            ProtocolError::CONFLICT.name(),
            ProtocolError::EXPIRED.name(),
            ProtocolError::ROLLBACK.name(),
            ProtocolError::UNAVAILABLE.name(),
            ProtocolError::INTERNAL.name(),
        ]
    );
    assert_eq!(EVENT_NAMES.len(), 9);
    assert!(classify_event_revision(10, 11).may_apply());
    assert!(classify_event_revision(10, 12).requires_resnapshot());
    assert!(classify_event_revision(10, 10).requires_resnapshot());
    let digest = Sha256::digest(LOCAL_API_SCHEMA_JSON);
    let actual_hash = digest
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    assert_eq!(actual_hash, LOCAL_API_SCHEMA_SHA256);
}

#[test]
fn generated_types_embed_the_exact_machine_contract() {
    let generated = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/src/api/generated.ts"
    ));
    let schema_prefix = "export const LOCAL_API_SCHEMA_JSON = `";
    let generated_schema = generated
        .split_once(schema_prefix)
        .and_then(|(_, remainder)| remainder.split_once('`'))
        .map(|(schema, _)| schema);
    assert_eq!(generated_schema, Some(LOCAL_API_SCHEMA_JSON));
    assert!(generated.contains(LOCAL_API_SCHEMA_SHA256));
    for command in COMMAND_NAMES {
        assert!(generated.contains(command));
    }
    for event in EVENT_NAMES {
        assert!(generated.contains(event));
    }
    let generated_types = generated.replacen(LOCAL_API_SCHEMA_JSON, "", 1);
    assert!(!generated_types.contains("authorized_via"));
    assert!(!generated_types.contains("private_key"));
    assert!(!generated_types.contains("session_bearer"));
    assert!(!generated_types.contains("invite_secret"));
}
