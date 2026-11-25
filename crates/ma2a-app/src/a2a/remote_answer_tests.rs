//! How remote answers that break Agent v1 reach an A2A client.

use ma2a_core::service::{
    AgentAccept, AgentReply, AgentTaskId, AgentTaskState, FrameKind, ServiceError,
};

use crate::{
    a2a::backend::{Backend, Refusal},
    test_daemon::{Daemon, Script, TestResult, answering, summary},
};

const TASK: AgentTaskId = AgentTaskId::from_bytes([1; 16]);

/// What a remote Cancel of [`TASK`] through a target answering `script` makes of it.
async fn cancelled(script: Script) -> TestResult<Result<(), Refusal>> {
    let daemon = Daemon::start("cancel", script)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    Ok(super::cancel(&backend, &route, &TASK.to_string())
        .await
        .map(drop))
}

#[tokio::test]
async fn a_cancel_takes_only_the_summary_of_an_ended_task() -> TestResult {
    let answered = |state| -> TestResult<Script> {
        let mut task = summary(TASK, "worker")?;
        task.state = state;
        answering(AgentAccept::Admitted, &AgentReply::Summary(task))
    };
    assert_eq!(
        cancelled(answered(AgentTaskState::Cancelled)?).await?,
        Ok(())
    );
    // A task still running did not end, which a cancel answers only once it did.
    assert_eq!(
        cancelled(answered(AgentTaskState::Running)?).await?,
        Err(Refusal::named("internal"))
    );
    Ok(())
}

/// What a remote Get of [`TASK`] through a target answering `script` makes of it.
async fn got(script: Script) -> TestResult<Result<(), Refusal>> {
    let daemon = Daemon::start("get", script)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    Ok(super::details(&backend, &route, &TASK.to_string())
        .await
        .map(drop))
}

#[tokio::test]
async fn a_target_breaking_the_frame_contract_is_an_internal_failure() -> TestResult {
    // A frame no answer starts with, or an accept that does not decode.
    let data = vec![(FrameKind::Data, vec![1, b'x'])];
    assert_eq!(got(data).await?, Err(Refusal::named("internal")));
    let garbled = vec![(FrameKind::Accept, vec![0xff])];
    assert_eq!(got(garbled).await?, Err(Refusal::named("internal")));
    // The target refusing the parameters stays a refusal of the parameters.
    let refused = vec![(FrameKind::Reject, vec![ServiceError::InvalidInput.code()])];
    assert_eq!(got(refused).await?, Err(Refusal::named("invalid_input")));
    Ok(())
}
