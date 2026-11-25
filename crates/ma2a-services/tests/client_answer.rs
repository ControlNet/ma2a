//! Which content-free answers a caller takes as the answer to the request it sent.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentId, AgentReply, AgentRequest, AgentTaskId, AgentTaskSummary,
        ProcessAccept, ProcessId, ProcessReply, ProcessRequest, ProcessSignal, ProcessSummary,
    },
};
use ma2a_services::client::answer;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn task(
    byte: u8,
    agent: &str,
) -> Result<AgentTaskSummary, Box<dyn std::error::Error + Send + Sync>> {
    let mut creator = [0_u8; 32];
    creator[0] = 1;
    Ok(AgentTaskSummary::new(
        (AgentTaskId::from_bytes([byte; 16]), AgentId::new(agent)?),
        (
            EndpointId::try_from(creator.as_slice())?,
            RequestId::try_from([9; 16].as_slice())?,
        ),
        1_000,
    ))
}

#[test]
fn a_task_page_moves_past_its_cursor_and_names_its_last_task() -> TestResult {
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
        &request,
        (admitted, &page(&[2, 3], Some(3))?)
    ));
    assert!(answer::agent(&request, (None, &page(&[], None)?)));
    for (bytes, next) in [
        (&[1, 2][..], None),
        (&[3, 2], None),
        (&[2, 3], Some(2)),
        (&[], Some(4)),
    ] {
        assert!(
            !answer::agent(&request, (admitted, &page(bytes, next)?)),
            "{bytes:?} {next:?}"
        );
    }
    let submitted = Some(AgentAccept::Submitted(
        AgentTaskId::from_bytes([2; 16]),
        false,
    ));
    assert!(!answer::agent(&request, (submitted, &page(&[2], None)?)));
    Ok(())
}

#[test]
fn a_process_answer_names_the_process_its_request_named() -> TestResult {
    let (a, b) = (
        ProcessId::from_bytes([1; 16]),
        ProcessId::from_bytes([2; 16]),
    );
    let request_id = RequestId::try_from([9; 16].as_slice())?;
    let summary = |id| ProcessSummary::new(id, request_id, 1);
    let of_a = ProcessReply::Summary(summary(a));
    let kill = |id| ProcessRequest::Signal(id, ProcessSignal::Kill);
    // A replayed outcome has no accept; a fresh one is admitted.
    assert!(answer::process(&kill(a), (None, &of_a)));
    assert!(answer::process(
        &kill(a),
        (Some(ProcessAccept::Admitted), &of_a)
    ));
    assert!(!answer::process(
        &kill(a),
        (Some(ProcessAccept::Started(a, false)), &of_a)
    ));
    assert!(!answer::process(&kill(b), (None, &of_a)));
    assert!(!answer::process(&ProcessRequest::List(None), (None, &of_a)));

    let listed = ProcessRequest::List(Some(a));
    let page = |ids: &[ProcessId], next| {
        ProcessReply::Page(ids.iter().map(|id| summary(*id)).collect(), next)
    };
    assert!(answer::process(&listed, (None, &page(&[b], Some(b)))));
    assert!(!answer::process(&listed, (None, &page(&[a], None))));
    Ok(())
}
