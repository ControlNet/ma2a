//! `agent watch` reconnecting after its watch was interrupted.

use ma2a_core::service::{
    AgentAccept, AgentEvent, AgentEventBody, AgentReply, AgentTaskId, AgentTaskState, FrameKind,
};

use crate::{
    AppError,
    cli::agents::{AgentCommand, TaskTarget},
    test_daemon::{
        Daemon, Script, TestResult, another_endpoint, refusing, summary, watch_completing,
        watch_lost,
    },
};

const TASK: AgentTaskId = AgentTaskId::from_bytes([1; 16]);

/// How `agent watch` of [`TASK`] after event 1 ends through the streams `opened` in turn.
async fn watched(opened: Vec<Script>) -> TestResult<Result<(), AppError>> {
    let daemon = Daemon::paging("watch", opened)?;
    let watch = AgentCommand::Watch {
        target: TaskTarget {
            agent: Some("worker".to_owned()),
            task: TASK.to_string(),
        },
        after: 1,
    };
    let remote = (&daemon.paths, daemon.endpoint.as_str());
    let watching = super::agent::run(remote, watch);
    Ok(tokio::time::timeout(std::time::Duration::from_secs(60), watching).await?)
}

#[tokio::test(start_paused = true)]
async fn a_watch_reconnects_while_reopening_it_fails_for_now() -> TestResult {
    // Given: a watch lost after event 2, whose first reopening finds no stream yet.
    let opened = vec![
        watch_lost()?,
        refusing("unavailable"),
        watch_completing(TASK)?,
    ];

    // When
    let ended = watched(opened).await?;

    // Then: the watch goes on to the task's end.
    assert!(ended.is_ok(), "{ended:?}");

    // A refusal that stays, or one before any watch ran, ends the watch at once.
    let opened = vec![
        watch_lost()?,
        refusing("invalid_input"),
        watch_completing(TASK)?,
    ];
    let ended = watched(opened).await?;
    assert!(
        matches!(&ended, Err(AppError::Rejected(name, _)) if name == "invalid_input"),
        "{ended:?}"
    );
    let ended = watched(vec![refusing("unavailable"), watch_completing(TASK)?]).await?;
    assert!(
        matches!(&ended, Err(AppError::Rejected(name, _)) if name == "unavailable"),
        "{ended:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_takes_no_end_of_a_task_another_endpoint_created() -> TestResult {
    // Given: a watch after event 1 that ends with event 2, and the Summary of that end names
    // another creator.
    let mut ended = summary(TASK, "worker")?;
    (ended.state, ended.last_seq) = (AgentTaskState::Completed, 2);
    ended.creator = another_endpoint()?;
    let mut event = vec![1];
    let completed = AgentEventBody::State(AgentTaskState::Completed, None);
    event.extend(AgentEvent::new(2, 2_000, completed).to_bytes()?);
    let watch = vec![
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

    // When
    let ended = watched(vec![watch]).await?;

    // Then
    assert!(ended.is_err(), "{ended:?}");
    Ok(())
}
