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

fn summary(state: AgentTaskState) -> TestResult<AgentTaskSummary> {
    // The identity point is a valid Ed25519 key.
    let mut creator = [0_u8; 32];
    creator[0] = 1;
    let mut summary = AgentTaskSummary::new(
        (AgentTaskId::from_bytes([7; 16]), AgentId::new("worker")?),
        (
            EndpointId::try_from(creator.as_slice())?,
            RequestId::try_from([9; 16].as_slice())?,
        ),
        1_000,
    );
    summary.state = state;
    Ok(summary)
}

fn event(seq: u32) -> TestResult<Vec<u8>> {
    let body = AgentEventBody::State(AgentTaskState::Running, None);
    Ok(AgentEvent::new(seq, 1_000, body).to_bytes()?)
}

/// Runs `watch` against a target that answers `accept`, then the events `seqs`, then
/// `outcome`; returns the result and the sequence numbers handed to the caller.
async fn watched(
    accept: AgentAccept,
    seqs: &[u32],
    outcome: &AgentReply,
) -> TestResult<(Result<(), ClientError>, Vec<u32>)> {
    let ((mut reader, _writer), (_target_reader, mut target)) = pipe();
    target.frame(FrameKind::Accept, &accept.encode()?).await?;
    for seq in seqs {
        target.data(Channel::Output, &event(*seq)?).await?;
    }
    let outcome = AgentReply::encode_outcome(&Ok(outcome.clone()))?;
    target.frame(FrameKind::Outcome, &outcome).await?;
    target.flush().await?;
    let mut seen = Vec::new();
    let watching = agent::watch(&mut reader, |event| seen.push(event.seq));
    let result = tokio::time::timeout(Duration::from_secs(30), watching).await?;
    Ok((result.map(|_| ()), seen))
}

const fn malformed(result: &Result<(), ClientError>) -> bool {
    matches!(result, Err(ClientError::Stream(ServiceError::InvalidInput)))
}

#[tokio::test]
async fn a_watch_ends_only_with_the_summary_of_an_ended_task() -> TestResult {
    let watching = AgentAccept::Watching(AgentTaskState::Running, 1);
    let ended = AgentReply::Summary(summary(AgentTaskState::Completed)?);
    let (result, seen) = watched(watching, &[1, 2], &ended).await?;
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
    let (result, _) = watched(AgentAccept::Admitted, &[], &ended).await?;
    assert!(malformed(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn watched_events_only_move_forward() -> TestResult {
    let ended = AgentReply::Summary(summary(AgentTaskState::Completed)?);
    // Pruned progress may leave a gap after `next`, but never before it.
    let from_five = AgentAccept::Watching(AgentTaskState::Running, 5);
    let (result, seen) = watched(from_five, &[7, 8], &ended).await?;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(seen, [7, 8]);
    for seqs in [&[4][..], &[5, 5], &[6, 5]] {
        let (result, _) = watched(from_five, seqs, &ended).await?;
        assert!(malformed(&result), "{seqs:?}: {result:?}");
    }
    Ok(())
}
