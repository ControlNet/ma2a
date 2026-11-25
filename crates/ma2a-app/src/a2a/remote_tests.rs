//! Remote answers and paging, and the deadline of one remote operation.

use ma2a_core::service::AgentTaskId;

use super::moves_past;

const fn id(byte: u8) -> AgentTaskId {
    AgentTaskId::from_bytes([byte; 16])
}

/// Whether a page listing `listed` and naming `next` moves past `after`.
fn moves(after: Option<u8>, listed: &[u8], next: Option<u8>) -> bool {
    let listed = listed.iter().map(|byte| id(*byte));
    moves_past(after.map(id).as_ref(), (listed, next.map(id)))
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

#[cfg(unix)]
mod through_the_daemon {
    use ma2a_core::{
        RequestId,
        service::{
            AgentAccept, AgentCard, AgentId, AgentMessage, AgentReply, AgentTaskDetails,
            AgentTaskState, FrameKind, MessagePart, MessageRole, ServiceError,
        },
    };
    use tokio::time::Instant;

    use super::{super::CALL_DEADLINE, id};
    use crate::{
        a2a::backend::{Backend, Refusal},
        test_daemon::{Daemon, Script, TestResult, answering, summary},
    };

    fn message() -> TestResult<AgentMessage> {
        Ok(AgentMessage::new(
            MessageRole::User,
            vec![MessagePart::Text("hi".into())],
        )?)
    }

    /// A target that answers `reply` from caller-side replay, with no accept.
    fn replayed(reply: &AgentReply) -> TestResult<Script> {
        let outcome = AgentReply::encode_outcome(&Ok(reply.clone()))?;
        Ok(vec![(FrameKind::Outcome, outcome)])
    }

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
        // The task of another submission, bound and answered in full, fresh or from replay.
        let mut another = summary(id(1), "worker")?;
        another.request_id = RequestId::try_from([8; 16].as_slice())?;
        let another = AgentReply::Summary(another);
        assert!(!submitted(bound(Ok(another.clone()))?).await?);
        assert!(!submitted(replayed(&another)?).await?);
        Ok(())
    }

    /// Whether a remote Send to task 1 through a target answering `script` succeeds.
    async fn sent(script: Script) -> TestResult<bool> {
        let daemon = Daemon::start("send", script)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let (task, request_id) = (id(1).to_string(), "0a".repeat(16));
        let sent = (task.as_str(), &message()?, request_id.as_str());
        Ok(super::super::send(&backend, &route, sent).await.is_ok())
    }

    #[tokio::test]
    async fn an_operation_takes_only_the_accept_it_is_answered_with() -> TestResult {
        let (get, send) = (details(1, "worker")?, brief(1, "worker")?);
        // A read is admitted; it never reserves a RequestId, so it is never answered from replay.
        assert!(got(answering(AgentAccept::Admitted, &get)?).await?);
        assert!(!got(replayed(&get)?).await?);
        assert!(!got(answering(AgentAccept::Submitted(id(1), false), &get)?).await?);
        let watching = AgentAccept::Watching(AgentTaskState::Queued, 1);
        assert!(!got(answering(watching, &get)?).await?);
        // A send is admitted, or answered from replay; it is never bound like a submit.
        assert!(sent(answering(AgentAccept::Admitted, &send)?).await?);
        assert!(sent(replayed(&send)?).await?);
        assert!(!sent(answering(AgentAccept::Submitted(id(1), false), &send)?).await?);
        Ok(())
    }

    /// What a remote Tasks through a target answering `script` lists.
    async fn listed(script: Script) -> TestResult<Result<usize, Refusal>> {
        let daemon = Daemon::start("tasks", script)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let listed = super::super::tasks(&backend, &route, None).await;
        Ok(listed.map(|(tasks, _)| tasks.len()))
    }

    #[tokio::test]
    async fn a_task_page_lists_only_the_routes_agent() -> TestResult {
        let page = |agent: &str| -> TestResult<Script> {
            let page = AgentReply::Page(vec![summary(id(1), agent)?], None);
            answering(AgentAccept::Admitted, &page)
        };
        assert_eq!(listed(page("worker")?).await?, Ok(1));
        // Another Agent's task, which this route would otherwise show as its own.
        assert_eq!(
            listed(page("helper")?).await?,
            Err(Refusal::named("internal"))
        );
        assert_eq!(
            listed(replayed(&AgentReply::Page(Vec::new(), None))?).await?,
            Err(Refusal::named("internal"))
        );
        Ok(())
    }

    /// One page of cards for Agents `names`, naming `next`.
    fn cards(names: &[&str], next: Option<&str>) -> TestResult<Script> {
        let card = |name: &str| -> TestResult<AgentCard> {
            Ok(AgentCard::new(AgentId::new(name)?, "", (true, 1))?)
        };
        let cards = names
            .iter()
            .map(|name| card(name))
            .collect::<TestResult<_>>()?;
        let next = next.map(AgentId::new).transpose()?;
        answering(AgentAccept::Admitted, &AgentReply::Cards(cards, next))
    }

    /// What a remote card lookup through a target answering `pages` in turn makes of it.
    async fn carded(pages: Vec<Script>) -> TestResult<Result<bool, Refusal>> {
        let daemon = Daemon::paging("card", pages)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let card = super::super::card(&backend, &route).await;
        Ok(card.map(|(_, available)| available))
    }

    #[tokio::test]
    async fn a_card_page_must_move_past_the_cursor_sent() -> TestResult {
        let internal = Err(Refusal::named("internal"));
        let first = cards(&["alpha"], Some("alpha"))?;
        assert_eq!(
            carded(vec![first.clone(), cards(&["worker"], None)?]).await?,
            Ok(true)
        );
        assert_eq!(
            carded(vec![cards(&["alpha"], None)?]).await?,
            Err(Refusal::named("unknown_agent"))
        );
        // Out of order, or a cursor that is not the page's last card, skips Agents.
        assert_eq!(
            carded(vec![cards(&["worker", "alpha"], None)?]).await?,
            internal
        );
        let skipping = cards(&["alpha"], Some("zulu"))?;
        assert_eq!(
            carded(vec![skipping, cards(&["worker"], None)?]).await?,
            internal
        );
        // A target still paging after the most pages read is not believed.
        let names: Vec<String> = (0..super::super::MAX_CARD_PAGES)
            .map(|page| format!("a{page:03}"))
            .collect();
        let pages = names
            .iter()
            .map(|name| cards(&[name.as_str()], Some(name.as_str())))
            .collect::<TestResult<Vec<_>>>()?;
        assert_eq!(carded(pages).await?, internal);
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
