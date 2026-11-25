//! Remote task paging, artifact references, and the deadline of one remote operation.

use ma2a_core::service::AgentTaskId;

use super::{matches_reference, moves_past};

const fn id(byte: u8) -> AgentTaskId {
    AgentTaskId::from_bytes([byte; 16])
}

/// Whether a page listing `listed` and naming `next` moves past `after`.
fn moves(after: Option<u8>, listed: &[u8], next: Option<u8>) -> bool {
    let listed = listed.iter().map(|byte| id(*byte));
    moves_past(after.map(id), (listed, next.map(id)))
}

#[test]
fn a_page_must_move_past_the_cursor_sent() {
    // The first page has nothing to move past.
    assert!(moves(None, &[1], Some(1)));
    assert!(moves(Some(1), &[2, 3], Some(3)));
    assert!(moves(Some(1), &[2], None));
    // The same cursor again, one behind it, or a task already listed.
    assert!(!moves(Some(3), &[4], Some(3)));
    assert!(!moves(Some(3), &[], Some(2)));
    assert!(!moves(Some(3), &[3], None));
    // Out of order or repeated, even on the first page.
    assert!(!moves(None, &[2, 1], None));
    assert!(!moves(None, &[1, 1], None));
    assert!(!moves(Some(1), &[3, 2], Some(2)));
    // A cursor that is not the page's last task skips tasks or repeats them.
    assert!(!moves(None, &[1, 2], Some(3)));
    assert!(!moves(None, &[1, 2], Some(1)));
    assert!(!moves(Some(1), &[], Some(2)));
}

#[test]
fn content_must_be_what_the_artifact_reference_names() {
    let hash = *blake3::hash(b"result").as_bytes();
    let hex = blake3::hash(b"result").to_hex();
    assert!(matches_reference((6, &hex), (6, &hash)));
    // Another size, another hash, or a reference that names no hash.
    assert!(!matches_reference((7, &hex), (6, &hash)));
    let other = blake3::hash(b"forged").to_hex();
    assert!(!matches_reference((6, &other), (6, &hash)));
    assert!(!matches_reference((6, ""), (6, &hash)));
}

#[cfg(unix)]
mod through_the_daemon {
    use ma2a_core::service::{
        AgentAccept, AgentMessage, AgentReply, AgentTaskDetails, MessagePart, MessageRole,
        ServiceError,
    };
    use tokio::time::Instant;

    use super::{super::CALL_DEADLINE, id};
    use crate::{
        a2a::backend::{Backend, Refusal},
        test_daemon::{Daemon, Script, TestResult, answering, summary},
    };

    fn details(task: u8, agent: &str) -> TestResult<AgentReply> {
        let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
        let details = AgentTaskDetails::new(summary(id(task), agent)?, input);
        Ok(AgentReply::Details(Box::new(details)))
    }

    fn brief(task: u8, agent: &str) -> TestResult<AgentReply> {
        Ok(AgentReply::Summary(summary(id(task), agent)?))
    }

    /// Whether a remote Get of task 1 through a target answering `script` succeeds.
    async fn got(script: Script) -> TestResult<bool> {
        let daemon = Daemon::start("get", script)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let task = id(1).to_string();
        Ok(super::super::details(&backend, &route, &task).await.is_ok())
    }

    /// Whether a remote Submit through a target answering `script` succeeds.
    async fn submitted(script: Script) -> TestResult<bool> {
        let daemon = Daemon::start("submit", script)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let message = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
        let request_id = "09".repeat(16);
        let sent = (&message, request_id.as_str());
        Ok(super::super::submit(&backend, &route, sent).await.is_ok())
    }

    #[tokio::test]
    async fn a_get_takes_only_the_details_of_the_task_it_named() -> TestResult {
        let admitted = |reply: TestResult<AgentReply>| answering(AgentAccept::Admitted, &reply?);
        assert!(got(admitted(details(1, "worker"))?).await?);
        // Another task of the same Agent, a task of another Agent, or a bare summary.
        assert!(!got(admitted(details(2, "worker"))?).await?);
        assert!(!got(admitted(details(1, "helper"))?).await?);
        assert!(!got(admitted(brief(1, "worker"))?).await?);
        Ok(())
    }

    #[tokio::test]
    async fn a_submit_takes_only_the_summary_of_the_task_it_was_bound_to() -> TestResult {
        let bound = |reply: TestResult<AgentReply>| {
            answering(AgentAccept::Submitted(id(1), false), &reply?)
        };
        assert!(submitted(bound(brief(1, "worker"))?).await?);
        // Details in place of the summary, which a stream would follow as no task at all.
        assert!(!submitted(bound(details(1, "worker"))?).await?);
        assert!(!submitted(bound(brief(2, "worker"))?).await?);
        assert!(!submitted(answering(AgentAccept::Admitted, &brief(1, "worker")?)?).await?);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn a_target_that_never_answers_fails_as_timed_out() -> TestResult {
        // Given: a target whose stream opens and then stays silent.
        let daemon = Daemon::start("silent", Vec::new())?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let (task, started) = (id(1).to_string(), Instant::now());

        // When
        let got = super::super::details(&backend, &route, &task);
        let answered = tokio::time::timeout(CALL_DEADLINE * 2, got).await;

        // Then: the operation's own deadline ended it, not the transport's.
        let timed_out = Refusal::named(ServiceError::TimedOut.name());
        assert_eq!(answered.ok(), Some(Err(timed_out)));
        assert_eq!(started.elapsed(), CALL_DEADLINE);
        Ok(())
    }
}
