//! Which content-free answers a caller takes as the answer to the request it sent.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentId, AgentMessage, AgentReply, AgentRequest, AgentSubmit,
        AgentTaskDetails, AgentTaskId, AgentTaskState, AgentTaskSummary, LogWindow, MessageRole,
        ProcessAccept, ProcessDetails, ProcessId, ProcessReply, ProcessRequest, ProcessSignal,
        ProcessSpec, ProcessStart, ProcessState, ProcessSummary,
    },
};
use ma2a_services::client::answer;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// The `RequestId` every request here was opened under.
fn opened() -> Result<RequestId, Box<dyn std::error::Error + Send + Sync>> {
    Ok(RequestId::try_from([9; 16].as_slice())?)
}

/// The Endpoint every request here was sent from, which created every task.
fn caller() -> Result<EndpointId, Box<dyn std::error::Error + Send + Sync>> {
    // The identity point is a valid Ed25519 key.
    let mut creator = [0_u8; 32];
    creator[0] = 1;
    Ok(EndpointId::try_from(creator.as_slice())?)
}

fn task(
    byte: u8,
    agent: &str,
) -> Result<AgentTaskSummary, Box<dyn std::error::Error + Send + Sync>> {
    Ok(AgentTaskSummary::new(
        (AgentTaskId::from_bytes([byte; 16]), AgentId::new(agent)?),
        (caller()?, opened()?),
        1_000,
    ))
}

#[test]
fn a_task_page_moves_past_its_cursor_and_names_its_last_task() -> TestResult {
    let (opened, caller) = (opened()?, caller()?);
    let worker = AgentId::new("worker")?;
    let after = AgentTaskId::from_bytes([1; 16]);
    let request = AgentRequest::Tasks(worker, Some(after));
    let page = |bytes: &[u8],
                next: Option<u8>|
     -> Result<AgentReply, Box<dyn std::error::Error + Send + Sync>> {
        let tasks = bytes
            .iter()
            .map(|byte| task(*byte, "worker"))
            .collect::<Result<_, _>>()?;
        Ok(AgentReply::Page(
            tasks,
            next.map(|byte| AgentTaskId::from_bytes([byte; 16])),
        ))
    };
    let admitted = Some(AgentAccept::Admitted);
    assert!(answer::agent(
        (&request, &opened, &caller),
        (admitted, &page(&[2, 3], Some(3))?)
    ));
    assert!(answer::agent(
        (&request, &opened, &caller),
        (admitted, &page(&[], None)?)
    ));
    for (bytes, next) in [
        (&[1, 2][..], None),
        (&[3, 2], None),
        (&[2, 3], Some(2)),
        (&[], Some(4)),
    ] {
        assert!(
            !answer::agent(
                (&request, &opened, &caller),
                (admitted, &page(bytes, next)?)
            ),
            "{bytes:?} {next:?}"
        );
    }
    let submitted = Some(AgentAccept::Submitted(
        AgentTaskId::from_bytes([2; 16]),
        false,
    ));
    assert!(!answer::agent(
        (&request, &opened, &caller),
        (submitted, &page(&[2], None)?)
    ));
    Ok(())
}

#[test]
fn a_process_answer_names_the_process_its_request_named() -> TestResult {
    let opened = opened()?;
    let (a, b) = (
        ProcessId::from_bytes([1; 16]),
        ProcessId::from_bytes([2; 16]),
    );
    let summary = |id| ProcessSummary::new(id, opened, 1);
    let of_a = ProcessReply::Summary(summary(a));
    let kill = |id| ProcessRequest::Signal(id, ProcessSignal::Kill);
    // A replayed outcome has no accept; a fresh one is admitted.
    assert!(answer::process((&kill(a), &opened), (None, &of_a)));
    assert!(answer::process(
        (&kill(a), &opened),
        (Some(ProcessAccept::Admitted), &of_a)
    ));
    assert!(!answer::process(
        (&kill(a), &opened),
        (Some(ProcessAccept::Started(a, false)), &of_a)
    ));
    assert!(!answer::process((&kill(b), &opened), (None, &of_a)));
    assert!(!answer::process(
        (&ProcessRequest::List(None), &opened),
        (None, &of_a)
    ));

    let listed = ProcessRequest::List(Some(a));
    let page = |ids: &[ProcessId], next| {
        ProcessReply::Page(ids.iter().map(|id| summary(*id)).collect(), next)
    };
    let admitted = Some(ProcessAccept::Admitted);
    assert!(answer::process(
        (&listed, &opened),
        (admitted, &page(&[b], Some(b)))
    ));
    assert!(!answer::process(
        (&listed, &opened),
        (admitted, &page(&[a], None))
    ));
    Ok(())
}

#[test]
fn a_read_is_answered_only_after_its_admission() -> TestResult {
    let (opened, caller) = (opened()?, caller()?);
    // A read reserves no RequestId, so no outcome of one is answered from replay.
    let worker = AgentId::new("worker")?;
    let task_a = AgentTaskId::from_bytes([1; 16]);
    let input = AgentMessage::text(MessageRole::User, "hi")?;
    let details = |summary| AgentTaskDetails::new(summary, input.clone());
    let reads = [
        (
            AgentRequest::Agents(None),
            AgentReply::Cards(Vec::new(), None),
        ),
        (
            AgentRequest::Get(worker.clone(), task_a),
            AgentReply::Details(Box::new(details(task(1, "worker")?))),
        ),
        (
            AgentRequest::Tasks(worker, None),
            AgentReply::Page(Vec::new(), None),
        ),
    ];
    for (request, reply) in &reads {
        let admitted = Some(AgentAccept::Admitted);
        assert!(
            answer::agent((request, &opened, &caller), (admitted, reply)),
            "{request:?}"
        );
        assert!(
            !answer::agent((request, &opened, &caller), (None, reply)),
            "{request:?}"
        );
    }

    let id = ProcessId::from_bytes([1; 16]);
    let summary = ProcessSummary::new(id, opened, 1);
    let reads = [
        (
            ProcessRequest::List(None),
            ProcessReply::Page(vec![summary.clone()], None),
        ),
        (
            ProcessRequest::Inspect(id),
            ProcessReply::Details(Box::new(ProcessDetails::new(
                summary,
                None,
                LogWindow::new(0, 0),
            ))),
        ),
    ];
    for (request, reply) in &reads {
        let admitted = Some(ProcessAccept::Admitted);
        assert!(
            answer::process((request, &opened), (admitted, reply)),
            "{request:?}"
        );
        assert!(
            !answer::process((request, &opened), (None, reply)),
            "{request:?}"
        );
    }
    Ok(())
}

#[test]
fn a_creation_is_answered_with_the_object_its_own_request_created() -> TestResult {
    let (opened, caller) = (opened()?, caller()?);
    let worker = AgentId::new("worker")?;
    let message = AgentMessage::text(MessageRole::User, "hi")?;
    let submit = AgentRequest::Submit(AgentSubmit::new(worker, message, None));
    let created = task(1, "worker")?;
    let mut another = created.clone();
    another.request_id = RequestId::try_from([8; 16].as_slice())?;
    let bound = |task: &AgentTaskSummary| Some(AgentAccept::Submitted(task.id, false));
    for accept in [bound(&created), None] {
        let reply = AgentReply::Summary(created.clone());
        assert!(answer::agent((&submit, &opened, &caller), (accept, &reply)));
        let reply = AgentReply::Summary(another.clone());
        assert!(!answer::agent(
            (&submit, &opened, &caller),
            (accept, &reply)
        ));
    }

    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start = ProcessRequest::Start(ProcessStart::new(spec, false, 2_097_152)?);
    let id = ProcessId::from_bytes([1; 16]);
    let started = ProcessReply::Summary(ProcessSummary::new(id, opened, 1));
    let another = ProcessSummary::new(id, RequestId::try_from([8; 16].as_slice())?, 1);
    let another = ProcessReply::Summary(another);
    for accept in [Some(ProcessAccept::Started(id, false)), None] {
        assert!(answer::process((&start, &opened), (accept, &started)));
        assert!(!answer::process((&start, &opened), (accept, &another)));
    }
    // A later operation on the object answers with the object's own creating RequestId.
    let send = AgentRequest::Send(
        AgentId::new("worker")?,
        created.id,
        AgentMessage::text(MessageRole::User, "more")?,
    );
    let summary = AgentReply::Summary(another_task(&created)?);
    assert!(answer::agent((&send, &opened, &caller), (None, &summary)));
    Ok(())
}

/// `task` as created by another request than [`OPENED`].
fn another_task(
    task: &AgentTaskSummary,
) -> Result<AgentTaskSummary, Box<dyn std::error::Error + Send + Sync>> {
    let mut another = task.clone();
    another.request_id = RequestId::try_from([8; 16].as_slice())?;
    Ok(another)
}

#[test]
fn an_end_is_answered_only_once_the_object_ended() -> TestResult {
    let (opened, caller) = (opened()?, caller()?);
    let worker = AgentId::new("worker")?;
    let mut summary = task(1, "worker")?;
    let cancel = AgentRequest::Cancel(worker.clone(), summary.id);
    let send = AgentRequest::Send(
        worker,
        summary.id,
        AgentMessage::text(MessageRole::User, "more")?,
    );
    let admitted = Some(AgentAccept::Admitted);
    for state in [AgentTaskState::Running, AgentTaskState::Waiting] {
        summary.state = state;
        let reply = AgentReply::Summary(summary.clone());
        assert!(
            !answer::agent((&cancel, &opened, &caller), (admitted, &reply)),
            "{state:?}"
        );
        assert!(
            answer::agent((&send, &opened, &caller), (admitted, &reply)),
            "{state:?}"
        );
    }
    summary.state = AgentTaskState::Cancelled;
    let reply = AgentReply::Summary(summary);
    assert!(answer::agent(
        (&cancel, &opened, &caller),
        (admitted, &reply)
    ));

    let id = ProcessId::from_bytes([1; 16]);
    let terminate = ProcessRequest::Terminate(id, 1_000);
    let signal = ProcessRequest::Signal(id, ProcessSignal::Kill);
    let mut summary = ProcessSummary::new(id, opened, 1);
    let admitted = Some(ProcessAccept::Admitted);
    summary.state = ProcessState::Running;
    let reply = ProcessReply::Summary(summary.clone());
    assert!(!answer::process((&terminate, &opened), (admitted, &reply)));
    assert!(answer::process((&signal, &opened), (admitted, &reply)));
    summary.state = ProcessState::Terminated;
    let reply = ProcessReply::Summary(summary);
    assert!(answer::process((&terminate, &opened), (admitted, &reply)));
    Ok(())
}

#[test]
fn a_task_answer_names_a_task_the_caller_created() -> TestResult {
    let (opened, caller) = (opened()?, caller()?);
    let worker = AgentId::new("worker")?;
    let mine = task(1, "worker")?;
    // Another Endpoint that sent the same message to the same route derived the same
    // RequestId, and its task is just as valid; it is still not this caller's.
    let mut theirs = mine.clone();
    theirs.creator = EndpointId::try_from(base_point().as_slice())?;
    let message = AgentMessage::text(MessageRole::User, "hi")?;
    let submit = AgentRequest::Submit(AgentSubmit::new(worker.clone(), message, None));
    let fresh = Some(AgentAccept::Submitted(mine.id, false));
    let listing = AgentRequest::Tasks(worker.clone(), None);
    let get = AgentRequest::Get(worker, mine.id);
    let input = AgentMessage::text(MessageRole::User, "hi")?;
    let admitted = Some(AgentAccept::Admitted);
    for (summary, taken) in [(&mine, true), (&theirs, false)] {
        let reply = AgentReply::Summary(summary.clone());
        // A fresh submit, and one answered from replay.
        assert_eq!(
            answer::agent((&submit, &opened, &caller), (fresh, &reply)),
            taken
        );
        assert_eq!(
            answer::agent((&submit, &opened, &caller), (None, &reply)),
            taken
        );
        let page = AgentReply::Page(vec![summary.clone()], None);
        assert_eq!(
            answer::agent((&listing, &opened, &caller), (admitted, &page)),
            taken
        );
        let details = AgentTaskDetails::new(summary.clone(), input.clone());
        let details = AgentReply::Details(Box::new(details));
        assert_eq!(
            answer::agent((&get, &opened, &caller), (admitted, &details)),
            taken
        );
    }
    Ok(())
}

/// A second valid Ed25519 key: the standard base point.
const fn base_point() -> [u8; 32] {
    let mut key = [0x66_u8; 32];
    key[0] = 0x58;
    key
}
