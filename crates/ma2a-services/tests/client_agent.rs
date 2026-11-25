//! The caller side of Agent v1 `watch` against a scripted target.
//!
//! The target here is a test script writing exact Agent v1 frames, not a Runtime.

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentEvent, AgentEventBody, AgentId, AgentReply, AgentTaskId, AgentTaskState,
        AgentTaskSummary, Channel, FrameKind, ServiceError,
    },
};
use ma2a_services::client::{ClientError, agent};
use support::{TestResult, pipe};

/// The task every watch here follows.
const TASK: AgentTaskId = AgentTaskId::from_bytes([7; 16]);

fn summary(state: AgentTaskState) -> TestResult<AgentTaskSummary> {
    summary_of(TASK, state)
}

/// The Endpoint every watch here is sent by, which created every task: the identity point, a
/// valid Ed25519 key.
fn caller() -> TestResult<EndpointId> {
    let mut caller = [0_u8; 32];
    caller[0] = 1;
    Ok(EndpointId::try_from(caller.as_slice())?)
}

fn summary_of(task: AgentTaskId, state: AgentTaskState) -> TestResult<AgentTaskSummary> {
    let mut summary = AgentTaskSummary::new(
        (task, AgentId::new("worker")?),
        (caller()?, RequestId::try_from([9; 16].as_slice())?),
        1_000,
    );
    summary.state = state;
    Ok(summary)
}

/// The summary of [`TASK`] completed with its newest event at `last_seq`.
fn ended_at(last_seq: u32) -> TestResult<AgentReply> {
    let mut ended = summary(AgentTaskState::Completed)?;
    ended.last_seq = last_seq;
    Ok(AgentReply::Summary(ended))
}

/// The events `seqs`: the task running, and completed at the last one.
fn events(seqs: &[u32]) -> Vec<(u32, AgentEventBody)> {
    let mut events: Vec<_> = seqs
        .iter()
        .map(|seq| (*seq, AgentEventBody::State(AgentTaskState::Running, None)))
        .collect();
    if let Some(last) = events.last_mut() {
        last.1 = AgentEventBody::State(AgentTaskState::Completed, None);
    }
    events
}

/// Runs `watch` of [`TASK`] after `after` against a target that answers `accept`, then the
/// events `seqs`, the last of them the task's end, then `outcome`; returns the result and
/// the sequence numbers handed to the caller.
async fn watched(
    watching: (u32, AgentAccept),
    seqs: &[u32],
    outcome: &AgentReply,
) -> TestResult<(Result<(), ClientError>, Vec<u32>)> {
    watched_events(watching, &events(seqs), outcome).await
}

/// Runs `watch` as [`watched`] does, with the event bodies the target sends.
async fn watched_events(
    (after, accept): (u32, AgentAccept),
    events: &[(u32, AgentEventBody)],
    outcome: &AgentReply,
) -> TestResult<(Result<(), ClientError>, Vec<u32>)> {
    let ((mut reader, _writer), (_target_reader, mut target)) = pipe();
    target.frame(FrameKind::Accept, &accept.encode()?).await?;
    for (seq, body) in events {
        let event = AgentEvent::new(*seq, 1_000, body.clone()).to_bytes()?;
        target.data(Channel::Output, &event).await?;
    }
    let outcome = AgentReply::encode_outcome(&Ok(outcome.clone()))?;
    target.frame(FrameKind::Outcome, &outcome).await?;
    target.flush().await?;
    let mut seen = Vec::new();
    let (caller, worker) = (caller()?, AgentId::new("worker")?);
    let watching = agent::watch(&mut reader, ((&caller, &worker, TASK), after), |event| {
        seen.push(event.seq);
    });
    let result = tokio::time::timeout(Duration::from_secs(30), watching).await?;
    Ok((result.map(|_| ()), seen))
}

const fn malformed(result: &Result<(), ClientError>) -> bool {
    matches!(result, Err(ClientError::Stream(ServiceError::InvalidInput)))
}

#[tokio::test]
async fn a_watch_ends_only_with_the_summary_of_an_ended_task() -> TestResult {
    let watching = (0, AgentAccept::Watching(AgentTaskState::Running, 1));
    let (result, seen) = watched(watching, &[1, 2], &ended_at(2)?).await?;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(seen, [1, 2]);

    // A task that has not ended, or an answer that is not a summary, is no end of a watch.
    let running = AgentReply::Summary(summary(AgentTaskState::Running)?);
    let (result, _) = watched(watching, &[], &running).await?;
    assert!(malformed(&result), "{result:?}");
    let page = AgentReply::Page(Vec::new(), None);
    let (result, _) = watched(watching, &[], &page).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_watch_must_be_accepted_as_one() -> TestResult {
    let ended = AgentReply::Summary(summary(AgentTaskState::Completed)?);
    let (result, _) = watched((0, AgentAccept::Admitted), &[], &ended).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn watched_events_only_move_forward() -> TestResult {
    let ended = ended_at(8)?;
    // Pruned progress may leave a gap after `next`, but never before it.
    let from_five = (4, AgentAccept::Watching(AgentTaskState::Running, 5));
    let (result, seen) = watched(from_five, &[7, 8], &ended).await?;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(seen, [7, 8]);
    for seqs in [&[4][..], &[5, 5], &[6, 5]] {
        let (result, _) = watched(from_five, seqs, &ended).await?;
        assert!(malformed(&result), "{seqs:?}: {result:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_watch_answers_the_cursor_and_task_it_was_asked_for() -> TestResult {
    let ended = AgentReply::Summary(summary(AgentTaskState::Completed)?);
    // A stale answer to a reconnect after 20 would hand back events already delivered.
    for next in [10, 20, 22] {
        let stale = (20, AgentAccept::Watching(AgentTaskState::Running, next));
        let (result, seen) = watched(stale, &[next], &ended).await?;
        assert!(malformed(&result), "{next}: {result:?}");
        assert!(seen.is_empty(), "{next}: {seen:?}");
    }
    // Another task's end is not this watch's end.
    let other = summary_of(AgentTaskId::from_bytes([8; 16]), AgentTaskState::Completed)?;
    let watching = (0, AgentAccept::Watching(AgentTaskState::Running, 1));
    let (result, _) = watched(watching, &[], &AgentReply::Summary(other)).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_watch_after_the_last_sequence_number_takes_no_event() -> TestResult {
    let ended = ended_at(u32::MAX)?;
    // The target saturates `next` at the maximum, which no event lies past.
    let last = (
        u32::MAX,
        AgentAccept::Watching(AgentTaskState::Running, u32::MAX),
    );
    let (result, seen) = watched(last, &[], &ended).await?;
    assert!(result.is_ok(), "{result:?}");
    assert!(seen.is_empty());
    let (result, seen) = watched(last, &[u32::MAX], &ended).await?;
    assert!(malformed(&result), "{result:?}");
    assert!(seen.is_empty(), "{seen:?}");
    Ok(())
}

#[tokio::test]
async fn a_watch_ends_only_after_the_last_event_of_the_task() -> TestResult {
    let from_five = (4, AgentAccept::Watching(AgentTaskState::Running, 5));
    // The task ended at 6, but its terminal event never came.
    let (result, _) = watched(from_five, &[], &ended_at(6)?).await?;
    assert!(malformed(&result), "{result:?}");
    let (result, _) = watched(from_five, &[5], &ended_at(6)?).await?;
    assert!(malformed(&result), "{result:?}");
    // A summary behind an event already delivered is not the end of that task either.
    let (result, _) = watched(from_five, &[5, 6], &ended_at(5)?).await?;
    assert!(malformed(&result), "{result:?}");

    // A watch from a cursor already past the end takes the summary without an event.
    for last_seq in [3, 4] {
        let (result, seen) = watched(from_five, &[], &ended_at(last_seq)?).await?;
        assert!(result.is_ok(), "{last_seq}: {result:?}");
        assert!(seen.is_empty(), "{last_seq}: {seen:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_watch_delivers_the_end_its_summary_reports() -> TestResult {
    let watching = (0, AgentAccept::Watching(AgentTaskState::Running, 1));
    // The last event the target sends is not the task's end, or another end than reported.
    let running = AgentEventBody::State(AgentTaskState::Running, None);
    let failed = AgentEventBody::State(AgentTaskState::Failed, None);
    for last in [
        running,
        AgentEventBody::Progress("working".to_owned()),
        failed,
    ] {
        let sent = [
            (1, AgentEventBody::State(AgentTaskState::Queued, None)),
            (2, last),
        ];
        let (result, seen) = watched_events(watching, &sent, &ended_at(2)?).await?;
        assert!(malformed(&result), "{sent:?}: {result:?}");
        assert_eq!(seen, [1, 2]);
    }
    // A task that ended has an event, so a summary with none is not its end.
    let (result, _) = watched(watching, &[], &ended_at(0)?).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_watch_ends_with_the_task_of_the_agent_it_named() -> TestResult {
    let mut other = summary(AgentTaskState::Completed)?;
    other.agent = AgentId::new("other")?;
    other.last_seq = 1;
    let watching = (0, AgentAccept::Watching(AgentTaskState::Running, 1));
    let (result, _) = watched(watching, &[1], &AgentReply::Summary(other)).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_watch_ends_with_a_task_its_caller_created() -> TestResult {
    // Given: an otherwise matching end of a task another Endpoint created.
    let mut key = [0x66_u8; 32];
    key[0] = 0x58;
    let mut other = summary(AgentTaskState::Completed)?;
    (other.creator, other.last_seq) = (EndpointId::try_from(key.as_slice())?, 1);
    let watching = (0, AgentAccept::Watching(AgentTaskState::Running, 1));

    // When
    let (result, _) = watched(watching, &[1], &AgentReply::Summary(other)).await?;

    // Then: it is not the end of the caller's own task.
    assert!(malformed(&result), "{result:?}");
    Ok(())
}
