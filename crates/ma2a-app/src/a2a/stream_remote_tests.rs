//! A stream following another Endpoint's task, through a stand-in daemon.

use ma2a_core::service::{
    AgentAccept, AgentEvent, AgentEventBody, AgentMessage, AgentReply, AgentTaskDetails,
    AgentTaskId, AgentTaskState, FrameKind, MessagePart, MessageRole,
};
use serde_json::json;

use super::{Lifecycle, Owned, respond};
use crate::{
    a2a::backend::Backend,
    test_daemon::{Daemon, Script, TestResult, answering, summary},
};

const TASK: AgentTaskId = AgentTaskId::from_bytes([1; 16]);

/// The text of a `SubscribeToTask` stream of a running task whose watch delivers its end at
/// event 2 and then answers with a summary of the task in `state`.
async fn subscribed(state: AgentTaskState) -> TestResult<String> {
    let mut running = summary(TASK, "worker")?;
    (running.state, running.last_seq) = (AgentTaskState::Running, 1);
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
    let details = AgentReply::Details(Box::new(AgentTaskDetails::new(running.clone(), input)));
    let completed = AgentEventBody::State(AgentTaskState::Completed, None);
    let mut event = vec![1];
    event.extend(AgentEvent::new(2, 2_000, completed).to_bytes()?);
    let mut ended = running;
    (ended.state, ended.last_seq) = (state, 2);
    let watch: Script = vec![
        (
            FrameKind::Accept,
            AgentAccept::Watching(AgentTaskState::Running, 2).encode()?,
        ),
        (FrameKind::Data, event),
        (
            FrameKind::Outcome,
            AgentReply::encode_outcome(&Ok(AgentReply::Summary(ended)))?,
        ),
    ];
    let daemon = Daemon::paging(
        "stream",
        vec![answering(AgentAccept::Admitted, &details)?, watch],
    )?;
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
