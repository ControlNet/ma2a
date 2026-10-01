//! The adapter protocol's exact JSON shape and the tool vocabulary.

use ma2a_agent::{
    protocol::{
        AgentToRuntime, Message, Origin, Part, ReportedState, Role, RuntimeToAgent, ToolError,
        decode_line, encode_line, is_call_id,
    },
    tools::{Tool, ToolFamily, TransferPut},
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_agent_line_has_one_exact_tagged_shape() -> TestResult {
    let lines = [
        (
            AgentToRuntime::Ready { protocol: 1 },
            json!({"t": "ready", "protocol": 1}),
        ),
        (
            AgentToRuntime::Ack { seq: 7 },
            json!({"t": "ack", "seq": 7}),
        ),
        (
            AgentToRuntime::State {
                task: "ab".to_owned(),
                state: ReportedState::Completed,
                message: Some(Message::agent_text("done")),
                error: None,
            },
            json!({"t": "state", "task": "ab", "state": "completed",
                   "message": {"role": "agent", "parts": [{"kind": "text", "text": "done"}]}}),
        ),
        (
            AgentToRuntime::Progress {
                task: "ab".to_owned(),
                text: "half".to_owned(),
            },
            json!({"t": "progress", "task": "ab", "text": "half"}),
        ),
        (
            AgentToRuntime::Tool {
                task: "ab".to_owned(),
                call: "c1".to_owned(),
                name: "files.stat".to_owned(),
                args: json!({"endpoint": "e"}),
            },
            json!({"t": "tool", "task": "ab", "call": "c1", "name": "files.stat",
                   "args": {"endpoint": "e"}}),
        ),
    ];
    for (message, expected) in lines {
        let line = encode_line(&message)?;
        assert_eq!(line.last(), Some(&b'\n'));
        let value: serde_json::Value = serde_json::from_slice(&line)?;
        assert_eq!(value, expected);
        let decoded: AgentToRuntime = decode_line(line.strip_suffix(b"\n").ok_or("no newline")?)?;
        assert_eq!(decoded, message);
    }
    Ok(())
}

#[test]
fn runtime_lines_round_trip_and_name_their_seq_and_task() -> TestResult {
    let deliveries = [
        RuntimeToAgent::Task {
            seq: 1,
            task: "t".to_owned(),
            from: Origin {
                endpoint: "e".to_owned(),
                agent: Some("planner".to_owned()),
            },
            message: Message {
                role: Role::User,
                parts: vec![
                    Part::Text {
                        text: "hi".to_owned(),
                    },
                    Part::Data {
                        data: json!({"k": [1, 2]}),
                    },
                ],
            },
        },
        RuntimeToAgent::Message {
            seq: 2,
            task: "t".to_owned(),
            message: Message::user_text("more"),
        },
        RuntimeToAgent::Cancel {
            seq: 3,
            task: "t".to_owned(),
        },
        RuntimeToAgent::ToolResult {
            seq: 4,
            task: "t".to_owned(),
            call: "c1".to_owned(),
            ok: None,
            error: Some(ToolError::new("tool_denied", "not allowed")),
        },
    ];
    for (index, delivery) in deliveries.into_iter().enumerate() {
        let line = encode_line(&delivery)?;
        // A carriage return before the newline is tolerated.
        let mut crlf = line.strip_suffix(b"\n").ok_or("no newline")?.to_vec();
        crlf.push(b'\r');
        let decoded: RuntimeToAgent = decode_line(&crlf)?;
        assert_eq!(decoded.seq(), u64::try_from(index)? + 1);
        assert_eq!(decoded.task(), "t");
        assert_eq!(decoded, delivery);
    }
    Ok(())
}

#[test]
fn unknown_fields_tags_and_kinds_are_refused() {
    let refused = [
        r#"{"t":"ready","protocol":1,"extra":true}"#,
        r#"{"t":"launch"}"#,
        r#"{"protocol":1}"#,
        r#"{"t":"state","task":"a","state":"queued"}"#,
        r#"{"t":"message","task":"a","message":{"role":"system","parts":[]}}"#,
        r#"{"t":"message","task":"a","message":{"role":"agent","parts":[{"kind":"file"}]}}"#,
        "not json",
        "",
    ];
    for line in refused {
        assert!(
            decode_line::<AgentToRuntime>(line.as_bytes()).is_err(),
            "{line}"
        );
    }
}

#[test]
fn call_ids_are_short_safe_tokens() {
    for valid in ["c1", "A.b_c-9", &"x".repeat(64)] {
        assert!(is_call_id(valid), "{valid}");
    }
    for invalid in ["", "has space", "slash/", &"x".repeat(65), "é"] {
        assert!(!is_call_id(invalid), "{invalid}");
    }
}

#[test]
fn every_tool_has_a_stable_name_and_family() {
    for tool in Tool::ALL {
        assert_eq!(Tool::from_name(tool.name()), Some(tool));
    }
    assert_eq!(Tool::from_name("shell.open"), None);
    assert_eq!(Tool::ArtifactPublish.family(), None);
    assert_eq!(Tool::AgentWait.family(), Some(ToolFamily::Agent));
    let mut bits = 0;
    for family in ToolFamily::ALL {
        assert_eq!(ToolFamily::from_name(family.name()), Some(family));
        assert_eq!(bits & family.bit(), 0);
        bits |= family.bit();
    }
    assert_eq!(
        ToolFamily::of_set(0b10_0101),
        [ToolFamily::Files, ToolFamily::Exec, ToolFamily::Agent]
    );
}

#[test]
fn tool_arguments_default_their_optional_fields_and_refuse_unknown_ones() -> TestResult {
    let put: TransferPut = serde_json::from_value(
        json!({"endpoint": "e", "root": "r", "path": "/a", "from": "a.txt"}),
    )?;
    assert!(!put.replace);
    assert!(
        serde_json::from_value::<TransferPut>(
            json!({"endpoint": "e", "root": "r", "path": "/a", "from": "a", "mode": 1})
        )
        .is_err()
    );
    Ok(())
}
