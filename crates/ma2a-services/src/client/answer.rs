//! Whether a content-free answer is the one its request asked for.
//!
//! A target could answer one request with the outcome of another: `get` of task A with the
//! details of task B, or `terminate` of a Process with `deleted`. The frame drivers only check
//! the frame contract, so a caller passes every answer through the check for what it sent
//! before it reports anything as that request's result.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentId, AgentReply, AgentRequest, AgentTaskId, AgentTaskSummary,
        ProcessAccept, ProcessReply, ProcessRequest,
    },
};

/// Whether `accept` and `reply` answer the content-free Agent operation `request`, sent by the
/// Endpoint `caller` and opened under the `RequestId` `opened`.
///
/// The reply must be the operation's own form, of the Agent and task it named: Cards for
/// `agents`, the Summary for `submit`, `send`, and `cancel`, Details for `get`, and a Page of
/// the named Agent's tasks for `tasks`. Every task it names is one `caller` created, since a
/// target binds a submission to its creator and request ID together and shows a caller only
/// its own tasks. The Summary of `submit` is of the task `opened` created, and that of
/// `cancel` of an ended task. The accept is Submitted for `submit`, binding the task the
/// Summary describes, and Admitted for the others; the outcome of a mutation answered from
/// replay has no accept, but a read reserves no `RequestId` and is never answered that way. A page lists ascending IDs strictly after the cursor sent and names its
/// own last one as the next cursor. `watch` and `artifact` have no such answer.
pub fn agent(
    (request, opened, caller): (&AgentRequest, &RequestId, &EndpointId),
    (accept, reply): (Option<AgentAccept>, &AgentReply),
) -> bool {
    let admitted = accept.map_or_else(
        || request.is_mutation(),
        |accept| accept == AgentAccept::Admitted,
    );
    let mine = |summary: &AgentTaskSummary| summary.creator == *caller;
    let names = |summary: &AgentTaskSummary, (agent, task): (&AgentId, &AgentTaskId)| {
        mine(summary) && summary.agent == *agent && summary.id == *task
    };
    match (request, reply) {
        (AgentRequest::Agents(after), AgentReply::Cards(cards, next)) => {
            let listed = cards.iter().map(|card| &card.id);
            admitted && ascending(after.as_ref(), (listed, next.as_ref()))
        }
        (AgentRequest::Submit(_), AgentReply::Summary(summary)) => {
            request.agent() == Some(&summary.agent)
                && mine(summary)
                && summary.request_id == *opened
                && match accept {
                    None => true,
                    Some(AgentAccept::Submitted(task, _)) => task == summary.id,
                    Some(_) => false,
                }
        }
        (AgentRequest::Send(agent, task, _), AgentReply::Summary(summary)) => {
            admitted && names(summary, (agent, task))
        }
        (AgentRequest::Cancel(agent, task), AgentReply::Summary(summary)) => {
            admitted && names(summary, (agent, task)) && summary.state.is_terminal()
        }
        (AgentRequest::Get(agent, task), AgentReply::Details(details)) => {
            admitted && names(&details.summary, (agent, task))
        }
        (AgentRequest::Tasks(agent, after), AgentReply::Page(tasks, next)) => {
            let listed = tasks.iter().map(|task| &task.id);
            admitted
                && tasks.iter().all(|task| mine(task) && task.agent == *agent)
                && ascending(after.as_ref(), (listed, next.as_ref()))
        }
        _ => false,
    }
}

/// Whether `accept` and `reply` answer the content-free Process operation `request`, opened
/// under the `RequestId` `opened`.
///
/// The reply must be the operation's own form, of the Process it named: the Summary for
/// `start`, `signal`, and `terminate`, a Page for `list`, Details for `inspect`, and Deleted
/// for `delete`. The Summary of `start` is of the Process `opened` created, and that of
/// `terminate` of an ended Process. The accept is Started for `start`, binding the Process the
/// Summary describes, and Admitted for the others; the outcome of a mutation answered from
/// replay has no accept, but a read reserves no `RequestId` and is never answered that way. A
/// page lists ascending IDs strictly after the cursor sent and names its own last one as the
/// next cursor. `logs`, `attach`, and `stdin` have no such answer.
pub fn process(
    (request, opened): (&ProcessRequest, &RequestId),
    (accept, reply): (Option<ProcessAccept>, &ProcessReply),
) -> bool {
    let admitted = accept.map_or_else(
        || request.is_mutation(),
        |accept| accept == ProcessAccept::Admitted,
    );
    match (request, reply) {
        (ProcessRequest::Start(_), ProcessReply::Summary(summary)) => {
            summary.request_id == *opened
                && match accept {
                    None => true,
                    Some(ProcessAccept::Started(id, _)) => id == summary.id,
                    Some(_) => false,
                }
        }
        (ProcessRequest::List(after), ProcessReply::Page(processes, next)) => {
            let listed = processes.iter().map(|process| &process.id);
            admitted && ascending(after.as_ref(), (listed, next.as_ref()))
        }
        (ProcessRequest::Inspect(id), ProcessReply::Details(details)) => {
            admitted && details.summary.id == *id
        }
        (ProcessRequest::Signal(id, _), ProcessReply::Summary(summary)) => {
            admitted && summary.id == *id
        }
        (ProcessRequest::Terminate(id, _), ProcessReply::Summary(summary)) => {
            admitted && summary.id == *id && summary.state.is_terminal()
        }
        (ProcessRequest::Delete(_), ProcessReply::Deleted) => admitted,
        _ => false,
    }
}

/// Whether a page lists IDs in ascending order strictly after `after`, and names its own
/// last one as `next` when more exist. A target answering otherwise would keep a listing
/// going forever, or put an entry where no cursor reaches it.
fn ascending<'a, T: Ord + 'a>(
    after: Option<&'a T>,
    (listed, next): (impl Iterator<Item = &'a T>, Option<&'a T>),
) -> bool {
    let (mut floor, mut last) = (after, None);
    for id in listed {
        if floor.is_some_and(|floor| id <= floor) {
            return false;
        }
        (floor, last) = (Some(id), Some(id));
    }
    next.is_none_or(|next| last == Some(next))
}
