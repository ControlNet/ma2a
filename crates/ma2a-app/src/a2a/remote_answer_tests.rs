//! How remote answers that break Agent v1 reach an A2A client, and how a follow reconnects.

use std::time::Duration;

use ma2a_core::service::{
    AgentAccept, AgentEvent, AgentEventBody, AgentMessage, AgentReply, AgentTaskDetails,
    AgentTaskId, AgentTaskState, FrameKind, MessagePart, MessageRole, ServiceError,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    a2a::backend::{Backend, Refusal},
    test_daemon::{
        Daemon, Script, TestResult, another_endpoint, answering, incompatible, refusing, summary,
        watch_completing, watch_lost,
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
    // The same task ID of another creator is not the task this caller cancelled.
    let mut theirs = summary(TASK, "worker")?;
    (theirs.state, theirs.creator) = (AgentTaskState::Cancelled, another_endpoint()?);
    let theirs = answering(AgentAccept::Admitted, &AgentReply::Summary(theirs))?;
    assert_eq!(cancelled(theirs).await?, Err(Refusal::named("internal")));
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
    // A Runtime replaced by one of another local API version stays that way.
    let opened = vec![watch_lost()?, incompatible(), watch_completing(TASK)?];
    let (seqs, ended) = followed(opened).await?;
    assert_eq!((seqs, ended), (vec![2], Err(Refusal::named("unavailable"))));
    Ok(())
}

#[tokio::test]
async fn a_remote_result_is_kept_in_full_however_long_it_escapes() -> TestResult {
    // Given: a completed task whose valid result grows past the local API's view bound once
    // escaped as JSON.
    let mut task = summary(TASK, "worker")?;
    task.state = AgentTaskState::Completed;
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
    let text = "\t".repeat(11_000);
    let mut full = AgentTaskDetails::new(task, input);
    full.result = Some(AgentMessage::new(
        MessageRole::Agent,
        vec![MessagePart::Text(text.clone())],
    )?);
    let answer = answering(AgentAccept::Admitted, &AgentReply::Details(Box::new(full)))?;
    let daemon = Daemon::start("get", answer)?;
    let (backend, route) = Backend::standing_in(&daemon)?;

    // When
    let got = super::details(&backend, &route, &TASK.to_string())
        .await
        .map_err(|refusal| refusal.0)?;

    // Then: the typed answer is all here, so nothing is left out.
    let content = got
        .pointer("/result/parts/0/content")
        .and_then(Value::as_str);
    assert_eq!(content, Some(text.as_str()));
    assert_eq!(got.get("result_elided"), Some(&Value::Bool(false)));
    Ok(())
}

/// How a wait for [`TASK`], running at event 1, to settle ends through a watch answering
/// `script`, or `None` while it still waits after a minute.
async fn waited(script: Script) -> TestResult<Option<Result<(), Refusal>>> {
    let daemon = Daemon::start("wait", script)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let served = crate::a2a::view::Served {
        backend: &backend,
        route: &route,
        base: "http://127.0.0.1",
    };
    let running =
        serde_json::json!({"task_id": TASK.to_string(), "state": "running", "last_seq": 1});
    let waiting = crate::a2a::view::wait(served, (&running, false), crate::a2a::view::settled);
    Ok(tokio::time::timeout(Duration::from_secs(60), waiting)
        .await
        .ok())
}

#[tokio::test(start_paused = true)]
async fn a_wait_ends_on_the_task_end_only_once_the_watch_confirmed_it() -> TestResult {
    // Given: a watch that delivers the task completed at event 2, then the Summary of a failure.
    let completed = {
        let mut data = vec![1];
        let body = AgentEventBody::State(AgentTaskState::Completed, None);
        data.extend(AgentEvent::new(2, 2_000, body).to_bytes()?);
        (FrameKind::Data, data)
    };
    let ending = |state| -> TestResult<Script> {
        let mut ended = summary(TASK, "worker")?;
        (ended.state, ended.last_seq) = (state, 2);
        let accept = AgentAccept::Watching(AgentTaskState::Running, 2).encode()?;
        let outcome = AgentReply::encode_outcome(&Ok(AgentReply::Summary(ended)))?;
        Ok(vec![
            (FrameKind::Accept, accept),
            completed.clone(),
            (FrameKind::Outcome, outcome),
        ])
    };

    // Then: the wait fails instead of taking an end the watch did not confirm.
    let contradicted = waited(ending(AgentTaskState::Failed)?).await?;
    assert_eq!(contradicted, Some(Err(Refusal::named("internal"))));
    // The same end confirmed by its Summary ends the wait.
    let confirmed = waited(ending(AgentTaskState::Completed)?).await?;
    assert_eq!(confirmed, Some(Ok(())));
    // An end the target never confirms does not end it.
    let mut unconfirmed = ending(AgentTaskState::Completed)?;
    unconfirmed.pop();
    assert_eq!(waited(unconfirmed).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_followed_agent_message_is_kept_in_full_however_long_it_escapes() -> TestResult {
    // Given: a watch delivering an Agent message that grows past the local API's view bound
    // once escaped as JSON, then the task's end.
    let text = "\t".repeat(11_000);
    let message = AgentMessage::new(MessageRole::Agent, vec![MessagePart::Text(text.clone())])?;
    let mut ended = summary(TASK, "worker")?;
    (ended.state, ended.last_seq) = (AgentTaskState::Completed, 3);
    let data = |seq, body| -> TestResult<(FrameKind, Vec<u8>)> {
        let mut data = vec![1];
        data.extend(AgentEvent::new(seq, 2_000, body).to_bytes()?);
        Ok((FrameKind::Data, data))
    };
    let watch = vec![
        (
            FrameKind::Accept,
            AgentAccept::Watching(AgentTaskState::Running, 2).encode()?,
        ),
        data(2, AgentEventBody::Message(message))?,
        data(3, AgentEventBody::State(AgentTaskState::Completed, None))?,
        (
            FrameKind::Outcome,
            AgentReply::encode_outcome(&Ok(AgentReply::Summary(ended)))?,
        ),
    ];
    let daemon = Daemon::start("follow", watch)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let (events, mut received) = mpsc::unbounded_channel();

    // When
    let task = TASK.to_string();
    let following = super::follow(&backend, (&route, (&task, 1)), &events);
    let ended = tokio::time::timeout(Duration::from_secs(60), following).await?;

    // Then: the typed event is all here, so its message is not left out.
    assert_eq!(ended, Ok(()));
    let first = received.recv().await.ok_or("no event")?;
    let content = first
        .pointer("/message/parts/0/content")
        .and_then(Value::as_str);
    assert!(content == Some(text.as_str()), "the message was left out");
    assert_eq!(first.get("message_elided"), Some(&Value::Bool(false)));
    Ok(())
}
