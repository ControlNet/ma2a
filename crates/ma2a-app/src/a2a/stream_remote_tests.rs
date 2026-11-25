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

#[tokio::test]
async fn a_stream_shows_an_agent_message_however_long_it_escapes() -> TestResult {
    // Given: a watch delivering an Agent message that grows past the local API's view bound
    // once escaped as JSON, then the task's end.
    let text = "\t".repeat(11_000);
    let message = AgentMessage::new(MessageRole::Agent, vec![MessagePart::Text(text)])?;
    let mut event = vec![1];
    event.extend(AgentEvent::new(2, 2_000, AgentEventBody::Message(message)).to_bytes()?);
    let events = vec![
        (FrameKind::Data, event),
        state_at(3, AgentTaskState::Completed)?,
    ];

    // When
    let text = streamed(vec![watch(1, events, (AgentTaskState::Completed, 3))?]).await?;

    // Then: one update carries the whole message.
    assert_eq!(
        text.matches("\\t").count(),
        11_000,
        "the message was left out"
    );
    assert!(
        text.contains("TASK_STATE_COMPLETED"),
        "the task did not end"
    );
    Ok(())
}

#[tokio::test]
async fn streamed_progress_and_messages_keep_the_last_state_change_timestamp() -> TestResult {
    // Given: a task whose state last changed at 1_000 ms, followed by progress, an Agent
    // message, a move to waiting at 4_000 ms, more progress, and completion at 6_000 ms.
    let frame = |seq, at_ms, body| -> TestResult<(FrameKind, Vec<u8>)> {
        let mut data = vec![1];
        data.extend(AgentEvent::new(seq, at_ms, body).to_bytes()?);
        Ok((FrameKind::Data, data))
    };
    let note = AgentMessage::new(MessageRole::Agent, vec![MessagePart::Text("note".into())])?;
    let events = vec![
        frame(2, 2_000, AgentEventBody::progress("halfway")?)?,
        frame(3, 3_000, AgentEventBody::Message(note))?,
        frame(
            4,
            4_000,
            AgentEventBody::State(AgentTaskState::Waiting, None),
        )?,
        frame(5, 5_000, AgentEventBody::progress("waiting")?)?,
        frame(
            6,
            6_000,
            AgentEventBody::State(AgentTaskState::Completed, None),
        )?,
    ];

    // When
    let text = streamed(vec![watch(1, events, (AgentTaskState::Completed, 6))?]).await?;

    // Then: every status timestamp matches the last state transition, not progress/message times.
    let stamps: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .filter_map(|response| {
            let status = response
                .pointer("/result/task/status")
                .or_else(|| response.pointer("/result/statusUpdate/status"))?;
            Some((
                status.get("state")?.as_str()?.to_owned(),
                status.get("timestamp")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let at = crate::a2a::time::format;
    assert_eq!(
        stamps,
        [
            ("TASK_STATE_WORKING".to_owned(), at(1_000)),
            ("TASK_STATE_WORKING".to_owned(), at(1_000)),
            ("TASK_STATE_WORKING".to_owned(), at(1_000)),
            ("TASK_STATE_INPUT_REQUIRED".to_owned(), at(4_000)),
            ("TASK_STATE_INPUT_REQUIRED".to_owned(), at(4_000)),
            ("TASK_STATE_COMPLETED".to_owned(), at(6_000)),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_fails_when_the_watch_ends_without_a_terminal_event() -> TestResult {
    // Given: a running task at event 1 whose watch sends no events and claims it ended at 1.
    let empty = streamed(vec![watch(1, Vec::new(), (AgentTaskState::Completed, 1))?]).await?;

    // Then: the stream refuses with -32603 instead of closing cleanly while still working.
    assert!(empty.contains("-32603"), "{empty}");
    assert!(!empty.contains("TASK_STATE_COMPLETED"), "{empty}");

    // Same for a continued stream (`sent_at = 1`, `shown = 3`) whose watch claims an end at
    // event 2 (`<= shown`).
    let mut running = summary(TASK, "worker")?;
    (running.state, running.last_seq) = (AgentTaskState::Running, 3);
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
    let details = AgentReply::Details(Box::new(AgentTaskDetails::new(running, input)));
    let scripts = vec![
        answering(AgentAccept::Admitted, &details)?,
        watch(
            1,
            vec![state_at(2, AgentTaskState::Completed)?],
            (AgentTaskState::Completed, 2),
        )?,
    ];
    let daemon = Daemon::paging("stream-stale-end", scripts)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let owned = Owned {
        backend,
        route,
        base: "http://127.0.0.1".to_owned(),
    };
    let lifecycle = Lifecycle {
        id: json!(1),
        method: "SendStreamingMessage",
        task: TASK.to_string(),
        history: None,
        until_waiting: true,
        sent_at: Some(1),
    };
    let body = respond(owned, lifecycle).into_body();
    let stale = String::from_utf8(axum::body::to_bytes(body, 1 << 20).await?.to_vec())?;
    assert!(stale.contains("-32603"), "{stale}");
    assert!(!stale.contains("TASK_STATE_COMPLETED"), "{stale}");
    Ok(())
}
