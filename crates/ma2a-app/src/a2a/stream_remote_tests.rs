//! A stream following another Endpoint's task, through a stand-in daemon.

use ma2a_core::service::{
    AgentAccept, AgentEvent, AgentEventBody, AgentMessage, AgentReply, AgentTaskDetails,
    AgentTaskId, AgentTaskState, FrameKind, MessagePart, MessageRole, ServiceError,
};
use serde_json::json;

use super::{Lifecycle, Owned, respond};
use crate::{
    a2a::backend::Backend,
    test_daemon::{Daemon, Script, TestResult, answering, summary},
};

const TASK: AgentTaskId = AgentTaskId::from_bytes([1; 16]);

/// A frame of event `seq` of a task, reaching `state`.
fn state_at(seq: u32, state: AgentTaskState) -> TestResult<(FrameKind, Vec<u8>)> {
    let mut event = vec![1];
    event.extend(AgentEvent::new(seq, 2_000, AgentEventBody::State(state, None)).to_bytes()?);
    Ok((FrameKind::Data, event))
}

/// A watch after event `after` accepted, then `events`, then the summary of the task in
/// `state` at event `last_seq`.
fn watch(
    after: u32,
    events: Vec<(FrameKind, Vec<u8>)>,
    (state, last_seq): (AgentTaskState, u32),
) -> TestResult<Script> {
    let mut ended = summary(TASK, "worker")?;
    (ended.state, ended.last_seq) = (state, last_seq);
    let accept = AgentAccept::Watching(AgentTaskState::Running, after + 1);
    let mut script = vec![(FrameKind::Accept, accept.encode()?)];
    script.extend(events);
    let outcome = AgentReply::encode_outcome(&Ok(AgentReply::Summary(ended)))?;
    script.push((FrameKind::Outcome, outcome));
    Ok(script)
}

/// The text of a `SubscribeToTask` stream of a task running at event 1, whose watch is
/// answered by the streams `opened` in turn.
async fn streamed(opened: Vec<Script>) -> TestResult<String> {
    let mut running = summary(TASK, "worker")?;
    (running.state, running.last_seq) = (AgentTaskState::Running, 1);
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
    let details = AgentReply::Details(Box::new(AgentTaskDetails::new(running, input)));
    let mut scripts = vec![answering(AgentAccept::Admitted, &details)?];
    scripts.extend(opened);
    let daemon = Daemon::paging("stream", scripts)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let owned = Owned {
        backend,
        route,
        base: "http://127.0.0.1".to_owned(),
    };
    let lifecycle = Lifecycle {
        id: json!(1),
        method: "SubscribeToTask",
        task: TASK.to_string(),
        history: None,
        until_waiting: false,
        sent_at: None,
    };
    let body = respond(owned, lifecycle).into_body();
    let text = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        axum::body::to_bytes(body, 1 << 20),
    )
    .await??;
    Ok(String::from_utf8(text.to_vec())?)
}

/// The text of a `SubscribeToTask` stream of a running task whose watch delivers its end at
/// event 2 and then answers with a summary of the task in `state`.
async fn subscribed(state: AgentTaskState) -> TestResult<String> {
    let completed = state_at(2, AgentTaskState::Completed)?;
    streamed(vec![watch(1, vec![completed], (state, 2))?]).await
}

#[tokio::test]
async fn a_task_end_is_shown_only_once_the_watch_confirmed_it() -> TestResult {
    // Given: a watch whose final summary agrees with the end it delivered.
    let confirmed = subscribed(AgentTaskState::Completed).await?;

    // Then: the stream shows the end.
    assert!(confirmed.contains("TASK_STATE_COMPLETED"), "{confirmed}");
    assert!(!confirmed.contains("\"error\""), "{confirmed}");

    // Given: a watch whose final summary contradicts it.
    let contradicted = subscribed(AgentTaskState::Failed).await?;

    // Then: the stream fails instead of showing an end the watch did not confirm.
    assert!(
        !contradicted.contains("TASK_STATE_COMPLETED"),
        "{contradicted}"
    );
    assert!(contradicted.contains("-32603"), "{contradicted}");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stream_shows_no_end_a_later_one_contradicts() -> TestResult {
    // Given: a watch that delivers the task completed at event 2 and failed at event 3, and
    // ends with the Summary of that failure.
    let completed = state_at(2, AgentTaskState::Completed)?;
    let failed = state_at(3, AgentTaskState::Failed)?;
    let twice = watch(
        1,
        vec![completed.clone(), failed],
        (AgentTaskState::Failed, 3),
    )?;

    // When
    let text = streamed(vec![twice]).await?;

    // Then: the stream fails instead of showing either end.
    assert!(!text.contains("TASK_STATE_COMPLETED"), "{text}");
    assert!(text.contains("-32603"), "{text}");

    // Given: a watch lost after it delivered the task completed at event 2, whose reconnect
    // ends with the Summary of the task failed at event 2.
    let lost = AgentReply::encode_outcome(&Err(ServiceError::Unavailable))?;
    let mut interrupted = watch(1, vec![completed], (AgentTaskState::Completed, 2))?;
    interrupted.pop();
    interrupted.push((FrameKind::Outcome, lost));
    let contradicting = watch(2, Vec::new(), (AgentTaskState::Failed, 2))?;

    // When
    let text = streamed(vec![interrupted, contradicting]).await?;

    // Then
    assert!(!text.contains("TASK_STATE_COMPLETED"), "{text}");
    assert!(text.contains("-32603"), "{text}");
    Ok(())
}
