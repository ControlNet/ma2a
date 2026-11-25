//! Canonical Agent v1 `open`, `accept`, `data`, and `outcome` codecs and their bounds.

use iroh_base::SecretKey;
use ma2a_core::{
    EndpointId, RequestId, ServiceKind,
    service::{
        AgentAccept, AgentCard, AgentEvent, AgentEventBody, AgentId, AgentMessage, AgentReply,
        AgentRequest, AgentSubmit, AgentTaskDetails, AgentTaskId, AgentTaskReason, AgentTaskState,
        AgentTaskSummary, ArtifactRef, ContentIdentity, MAX_MESSAGE_BYTES, MAX_MESSAGE_PARTS,
        MessagePart, MessageRole, OpenBody, OpenHeader, ServiceAccess, ServiceError, admission,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn endpoint(seed: u8) -> EndpointId {
    SecretKey::from_bytes(&[seed; 32]).public().into()
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::try_from([0x72; 16].as_slice())?)
}

const fn task() -> AgentTaskId {
    AgentTaskId::from_bytes([0x73; 16])
}

fn agent() -> TestResult<AgentId> {
    Ok(AgentId::new("research-1")?)
}

fn message() -> TestResult<AgentMessage> {
    Ok(AgentMessage::new(
        MessageRole::User,
        vec![
            MessagePart::Text("line one\nline two".to_owned()),
            MessagePart::Data(r#"{"depth":2}"#.to_owned()),
        ],
    )?)
}

fn roundtrip(body: AgentRequest) -> TestResult {
    let header = OpenHeader::new(request_id()?, endpoint(0x71), OpenBody::Agent(body));
    let bytes = header.encode()?;
    let decoded = OpenHeader::decode(&bytes)?;
    assert_eq!(decoded, header);
    assert_eq!(decoded.encode()?, bytes);
    assert_eq!(decoded.body().kind(), ServiceKind::Agent);
    Ok(())
}

#[test]
fn agent_names_are_a_closed_routing_form() -> TestResult {
    for valid in ["a", "helper", "research-1", "a1-b2"] {
        assert_eq!(AgentId::new(valid)?.as_str(), valid);
    }
    let too_long = "a".repeat(64);
    for invalid in [
        "",
        "1agent",
        "-x",
        "x-",
        "Agent",
        "a_b",
        "a b",
        too_long.as_str(),
    ] {
        assert!(AgentId::new(invalid).is_err(), "{invalid:?}");
    }
    let id = task();
    assert_eq!(AgentTaskId::parse(&id.to_string())?, id);
    assert!(AgentTaskId::parse("73").is_err());
    Ok(())
}

#[test]
fn every_operation_roundtrips_and_names_its_exact_agent() -> TestResult {
    let operations = [
        AgentRequest::Agents(None),
        AgentRequest::Agents(Some(agent()?)),
        AgentRequest::Submit(AgentSubmit::new(agent()?, message()?, None)),
        AgentRequest::Submit(AgentSubmit::new(
            agent()?,
            message()?,
            Some(AgentId::new("caller")?),
        )),
        AgentRequest::Send(agent()?, task(), message()?),
        AgentRequest::Get(agent()?, task()),
        AgentRequest::Tasks(agent()?, None),
        AgentRequest::Tasks(agent()?, Some(task())),
        AgentRequest::Watch(agent()?, task(), 7),
        AgentRequest::Cancel(agent()?, task()),
        AgentRequest::Artifact(agent()?, task(), "report.txt".to_owned(), 0, None),
        AgentRequest::Artifact(
            agent()?,
            task(),
            "report.txt".to_owned(),
            10,
            Some(ContentIdentity::new(20, 0)),
        ),
    ];
    for operation in operations {
        let expected = operation
            .agent()
            .map_or(ServiceAccess::AgentAdmission, |agent| {
                ServiceAccess::Agent(agent.clone())
            });
        assert_eq!(operation.access(), expected);
        let mutation = matches!(
            operation,
            AgentRequest::Submit(_) | AgentRequest::Send(..) | AgentRequest::Cancel(..)
        );
        assert_eq!(operation.is_mutation(), mutation);
        roundtrip(operation)?;
    }
    assert_eq!(
        admission(ServiceKind::Agent),
        Some(ServiceAccess::AgentAdmission)
    );
    Ok(())
}

#[test]
fn a_resumed_artifact_download_must_name_its_identity() -> TestResult {
    let resumed = AgentRequest::Artifact(agent()?, task(), "out.bin".to_owned(), 1, None);
    let header = OpenHeader::new(request_id()?, endpoint(0x71), OpenBody::Agent(resumed));
    assert_eq!(
        OpenHeader::decode(&header.encode()?),
        Err(ma2a_core::ProtocolError::INVALID_INPUT)
    );
    Ok(())
}

#[test]
fn messages_are_bounded_and_keep_newlines() -> TestResult {
    let full = "x".repeat(MAX_MESSAGE_BYTES);
    let maximal = AgentMessage::text(MessageRole::Agent, &full)?;
    assert_eq!(AgentMessage::from_bytes(&maximal.to_bytes()?)?, maximal);
    let header = OpenHeader::new(
        request_id()?,
        endpoint(0x71),
        OpenBody::Agent(AgentRequest::Submit(AgentSubmit::new(
            agent()?,
            AgentMessage::text(MessageRole::User, &full)?,
            Some(AgentId::new(&"z".repeat(63))?),
        ))),
    );
    // A maximal submission still fits in its header frame.
    assert_eq!(OpenHeader::decode(&header.encode()?)?, header);
    let over = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert!(AgentMessage::text(MessageRole::User, &over).is_err());
    assert!(AgentMessage::new(MessageRole::User, Vec::new()).is_err());
    let many = vec![MessagePart::Text("p".to_owned()); MAX_MESSAGE_PARTS + 1];
    assert!(AgentMessage::new(MessageRole::User, many).is_err());
    assert!(AgentMessage::text(MessageRole::User, "nul\0").is_err());
    assert_eq!(message()?.joined_text(), "line one\nline two");
    Ok(())
}

fn summary() -> TestResult<AgentTaskSummary> {
    let mut summary =
        AgentTaskSummary::new((task(), agent()?), (endpoint(0x74), request_id()?), 1_000);
    summary.from_agent = Some(AgentId::new("caller")?);
    summary.state = AgentTaskState::Failed;
    summary.reason = Some(AgentTaskReason::AgentLost);
    summary.times = (1_000, Some(1_100), Some(1_200));
    summary.last_seq = 9;
    Ok(summary)
}

fn artifact() -> TestResult<ArtifactRef> {
    Ok(ArtifactRef::new(
        ("report.txt", "text/plain"),
        (42, [0x75; 32]),
    )?)
}

#[test]
fn accepts_and_outcomes_roundtrip() -> TestResult {
    for accept in [
        AgentAccept::Submitted(task(), true),
        AgentAccept::Watching(AgentTaskState::Waiting, 3),
        AgentAccept::Admitted,
    ] {
        assert_eq!(AgentAccept::decode(&accept.encode()?)?, accept);
    }
    let mut details = AgentTaskDetails::new(summary()?, message()?);
    details.result = Some(AgentMessage::text(MessageRole::Agent, "done")?);
    details.error = Some("upstream\nfailed".to_owned());
    details.progress = Some("50%".to_owned());
    details.artifacts = vec![artifact()?];
    let card = AgentCard::new(agent()?, "Finds things.", (true, 4))?;
    let outcomes = [
        Ok(AgentReply::Cards(vec![card], Some(agent()?))),
        Ok(AgentReply::Cards(Vec::new(), None)),
        Ok(AgentReply::Summary(summary()?)),
        Ok(AgentReply::Details(Box::new(details))),
        Ok(AgentReply::Page(vec![summary()?], Some(task()))),
        Err(ServiceError::UnknownAgent),
        Err(ServiceError::TaskEnded),
    ];
    for outcome in outcomes {
        let bytes = AgentReply::encode_outcome(&outcome)?;
        assert_eq!(AgentReply::decode_outcome(&bytes)?, outcome);
    }
    assert!(AgentCard::new(agent()?, "", (false, 0)).is_err());
    assert!(AgentCard::new(agent()?, &"d".repeat(257), (false, 1)).is_err());
    Ok(())
}

#[test]
fn events_roundtrip_and_only_progress_is_prunable() -> TestResult {
    let bodies = [
        AgentEventBody::State(AgentTaskState::Running, None),
        AgentEventBody::State(AgentTaskState::Cancelled, Some(AgentTaskReason::Requested)),
        AgentEventBody::Message(message()?),
        AgentEventBody::progress("step 2 of 5")?,
        AgentEventBody::Artifact(artifact()?),
    ];
    for (index, body) in bodies.into_iter().enumerate() {
        let prunable = body.is_progress();
        let event = AgentEvent::new(u32::try_from(index)? + 1, 5_000, body);
        assert_eq!(AgentEvent::from_bytes(&event.to_bytes()?)?, event);
        assert_eq!(prunable, index == 3);
    }
    let zero = AgentEvent::new(0, 1, AgentEventBody::progress("x")?);
    assert!(AgentEvent::from_bytes(&zero.to_bytes()?).is_err());
    assert!(AgentEventBody::progress(&"p".repeat(1_025)).is_err());
    Ok(())
}

#[test]
fn artifact_names_and_media_types_are_closed() {
    assert!(ArtifactRef::new(("out.tar.gz", "application/gzip"), (1, [0; 32])).is_ok());
    for name in ["", ".hidden", "a/b", "a b", "../x"] {
        assert!(ArtifactRef::new((name, "text/plain"), (1, [0; 32])).is_err());
    }
    for media in ["", "text", "/plain", "text/", "text/plain; charset=utf-8"] {
        assert!(ArtifactRef::new(("a", media), (1, [0; 32])).is_err());
    }
}

#[test]
fn task_states_follow_the_lifecycle_only() {
    use AgentTaskState::{Cancelled, Completed, Failed, Queued, Running, Waiting};
    assert!(Queued.permits(Running) && Queued.permits(Cancelled) && Queued.permits(Failed));
    assert!(!Queued.permits(Completed) && !Queued.permits(Waiting));
    assert!(Running.permits(Waiting) && Waiting.permits(Running));
    for terminal in [Completed, Failed, Cancelled] {
        assert!(terminal.is_terminal());
        for next in AgentTaskState::ALL {
            assert!(!terminal.permits(next));
        }
    }
    for state in AgentTaskState::ALL {
        assert_eq!(AgentTaskState::from_name(state.name()), Some(state));
        assert_eq!(
            AgentTaskState::from_code(u64::from(state.code())).ok(),
            Some(state)
        );
    }
}
