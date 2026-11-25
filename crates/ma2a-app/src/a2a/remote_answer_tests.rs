//! How remote answers that break Agent v1 reach an A2A client, and how a follow reconnects.

use std::time::Duration;

use ma2a_core::service::{
    AgentAccept, AgentReply, AgentTaskId, AgentTaskState, FrameKind, ServiceError,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    a2a::backend::{Backend, Refusal},
    test_daemon::{
        Daemon, Script, TestResult, answering, refusing, summary, watch_completing, watch_lost,
    },
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

/// The events a follow of [`TASK`] after event 1 delivers through the streams `opened` in
/// turn, and how it ended.
async fn followed(opened: Vec<Script>) -> TestResult<(Vec<u64>, Result<(), Refusal>)> {
    let daemon = Daemon::paging("follow", opened)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let (events, mut received) = mpsc::unbounded_channel();
    let task = TASK.to_string();
    let following = super::follow(&backend, (&route, (&task, 1)), &events);
    let ended = tokio::time::timeout(Duration::from_secs(60), following).await?;
    drop(events);
    let mut seqs = Vec::new();
    while let Some(event) = received.recv().await {
        seqs.extend(event.get("seq").and_then(Value::as_u64));
    }
    Ok((seqs, ended))
}

#[tokio::test(start_paused = true)]
async fn a_follow_reconnects_while_reopening_its_watch_fails_for_now() -> TestResult {
    // Given: a watch lost after event 2, whose first reopening finds no stream yet.
    let opened = vec![
        watch_lost()?,
        refusing("unavailable"),
        watch_completing(TASK)?,
    ];

    // When
    let (seqs, ended) = followed(opened).await?;

    // Then: the follow goes on from event 2 to the end.
    assert_eq!((seqs, ended), (vec![2, 3], Ok(())));

    // A refusal that stays, or one before any watch ran, ends the follow at once.
    let opened = vec![
        watch_lost()?,
        refusing("invalid_input"),
        watch_completing(TASK)?,
    ];
    let (_, ended) = followed(opened).await?;
    assert_eq!(ended, Err(Refusal::named("invalid_input")));
    let (seqs, ended) = followed(vec![refusing("unavailable"), watch_completing(TASK)?]).await?;
    assert_eq!(
        (seqs, ended),
        (Vec::new(), Err(Refusal::named("unavailable")))
    );
    Ok(())
}
