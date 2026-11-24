//! The Agent family host: one operation per stream, the caller's own tasks only.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentCard, AgentId, AgentReply, AgentReplyOutcome, AgentRequest,
        AgentTaskReason, FrameKind, MAX_AGENT_PAGE, MAX_TASK_PAGE, ServiceAccess, ServiceError,
    },
};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::AgentManager;

/// Serves one decoded Agent operation for `peer`.
pub(crate) async fn serve<R, W>(
    manager: &AgentManager,
    ((peer, request_id), request, stop): (
        (EndpointId, RequestId),
        AgentRequest,
        &CancellationToken,
    ),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let viewer = |agent| (Some(peer), Some(agent));
    match request {
        AgentRequest::Agents(after) => answer(writer, manager.discover(peer, after).await).await,
        AgentRequest::Submit(submit) => match manager.submit((peer, request_id), &submit).await {
            Ok((record, existing)) => {
                let accept = AgentAccept::Submitted(record.summary.id, existing);
                respond(writer, accept, Ok(AgentReply::Summary(record.summary))).await
            }
            Err(error) => refuse(writer, error).await,
        },
        AgentRequest::Send(agent, task, message) => {
            let sent = manager.send(viewer(&agent), (task, message)).await;
            answer(
                writer,
                sent.map(|record| AgentReply::Summary(record.summary)),
            )
            .await
        }
        AgentRequest::Get(agent, task) => {
            let details = manager.details(viewer(&agent), task).await;
            answer(
                writer,
                details.map(|details| AgentReply::Details(Box::new(details))),
            )
            .await
        }
        AgentRequest::Tasks(agent, after) => {
            let page = MAX_TASK_PAGE + 1;
            let listed = manager
                .tasks(
                    (Some(peer), Some(agent)),
                    (after, u32::try_from(page).unwrap_or(33)),
                )
                .await;
            let reply = listed.map(|(_, mut records)| {
                let next = (records.len() > MAX_TASK_PAGE).then(|| {
                    records.truncate(MAX_TASK_PAGE);
                    records.last().map(|record| record.summary.id)
                });
                let summaries = records.into_iter().map(|record| record.summary).collect();
                AgentReply::Page(summaries, next.flatten())
            });
            answer(writer, reply).await
        }
        AgentRequest::Watch(agent, task, after) => {
            manager
                .watch((viewer(&agent), stop, (task, after)), (reader, writer))
                .await
        }
        AgentRequest::Cancel(agent, task) => {
            let reason = AgentTaskReason::Requested;
            let cancelled = manager.cancel(viewer(&agent), (task, reason)).await;
            answer(
                writer,
                cancelled.map(|record| AgentReply::Summary(record.summary)),
            )
            .await
        }
        AgentRequest::Artifact(agent, task, name, offset, identity) => {
            let operation = (viewer(&agent), (task, name), (offset, identity));
            manager
                .serve_artifact((operation, stop), (reader, writer))
                .await
        }
    }
}

impl AgentManager {
    /// Returns one page of the Agents `peer` may use, in name order.
    async fn discover(
        &self,
        peer: EndpointId,
        after: Option<AgentId>,
    ) -> Result<AgentReply, ServiceError> {
        let (_, agents) = self.list().await?;
        let mut cards = Vec::new();
        let mut more = false;
        for (record, _) in agents {
            let id = record.definition.id.clone();
            if after.as_ref().is_some_and(|after| id <= *after) {
                continue;
            }
            let access = ServiceAccess::Agent(id.clone());
            if self
                .0
                .store
                .authorize_service((self.0.local, peer), access)
                .await
                .is_err()
            {
                continue;
            }
            if cards.len() == MAX_AGENT_PAGE {
                more = true;
                break;
            }
            let available = (record.enabled, record.definition.max_tasks);
            let card = AgentCard::new(id, &record.definition.description, available)
                .map_err(|_| ServiceError::Internal)?;
            cards.push(card);
        }
        let next = more
            .then(|| cards.last().map(|card: &AgentCard| card.id.clone()))
            .flatten();
        Ok(AgentReply::Cards(cards, next))
    }
}

/// Longest wait to write the frame that opens or ends an operation, so a caller that stops
/// reading cannot hold the host.
const FRAME_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Refuses an operation before any effect.
pub(super) async fn refuse<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    error: ServiceError,
) -> OperationReport {
    let _sent = tokio::time::timeout(FRAME_GRACE, writer.reject(error)).await;
    OperationReport::new(OperationEnd::Rejected(error))
}

/// Writes `accept` within [`FRAME_GRACE`]; `false` when it was not written in time.
pub(super) async fn write_accept<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    payload: &[u8],
) -> bool {
    matches!(
        tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Accept, payload)).await,
        Ok(Ok(()))
    )
}

/// Sends the terminal outcome of an accepted operation, bounded against a stalled peer.
pub(super) async fn finish<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    outcome: AgentReplyOutcome,
) -> OperationReport {
    let end = match &outcome {
        Ok(_) => OperationEnd::Completed,
        Err(error) => OperationEnd::Failed(*error),
    };
    let Ok(payload) = AgentReply::encode_outcome(&outcome) else {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Internal));
    };
    let wrote = tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Outcome, &payload)).await;
    match wrote {
        Ok(Ok(())) => OperationReport::new(end),
        Ok(Err(_)) => OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled)),
        Err(_) => OperationReport::new(OperationEnd::Failed(ServiceError::TimedOut)),
    }
}

/// Sends `accept` and the outcome.
async fn respond<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    accept: AgentAccept,
    outcome: AgentReplyOutcome,
) -> OperationReport {
    let Ok(accept) = accept.encode() else {
        return refuse(writer, ServiceError::Internal).await;
    };
    if !write_accept(writer, &accept).await {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
    }
    finish(writer, outcome).await
}

/// Answers a decided operation: its refusal, or `accept` and its outcome.
async fn answer<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    reply: Result<AgentReply, ServiceError>,
) -> OperationReport {
    match reply {
        Ok(reply) => respond(writer, AgentAccept::Admitted, Ok(reply)).await,
        // A cancellation whose end is unknown may already have had its effect.
        Err(ServiceError::OutcomeUnknown) => {
            let error = Err(ServiceError::OutcomeUnknown);
            respond(writer, AgentAccept::Admitted, error).await
        }
        Err(error) => refuse(writer, error).await,
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{AgentAccept, AgentReply, ServiceError};
    use ma2a_services::{FrameWriter, OperationEnd};

    #[tokio::test(start_paused = true)]
    async fn a_caller_that_never_reads_cannot_hold_an_answer() {
        // A stream with no room, whose reader never drains it.
        let (_caller, host) = tokio::io::duplex(1);
        let mut writer = FrameWriter::new(host);
        let started = tokio::time::Instant::now();

        let refused = super::refuse(&mut writer, ServiceError::UnknownTask).await;
        let answered = super::respond(
            &mut writer,
            AgentAccept::Admitted,
            Ok(AgentReply::Cards(Vec::new(), None)),
        )
        .await;

        assert_eq!(
            refused.end,
            OperationEnd::Rejected(ServiceError::UnknownTask)
        );
        assert_eq!(answered.end, OperationEnd::Failed(ServiceError::Cancelled));
        assert!(started.elapsed() <= super::FRAME_GRACE * 3);
    }
}
