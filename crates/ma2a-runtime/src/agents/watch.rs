//! `watch`: a task's retained events after a sequence number, then each new one, until
//! the task ended and every event before its end was sent.

use ma2a_core::service::{AgentAccept, AgentReply, AgentTaskId, Channel, FrameKind, ServiceError};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{
    AgentManager,
    host::{finish, refuse},
    tasks::Viewer,
};

/// Most events read from the Store at once.
const EVENT_BATCH: u32 = 64;

impl AgentManager {
    pub(super) async fn watch<R, W>(
        &self,
        (viewer, stop, (task, mut after)): (Viewer<'_>, &CancellationToken, (AgentTaskId, u32)),
        (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    ) -> OperationReport
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        let record = match self.visible_task(viewer, task).await {
            Ok(record) => record,
            Err(error) => return refuse(writer, error).await,
        };
        let accept = AgentAccept::Watching(record.summary.state, after.saturating_add(1));
        let Ok(accept) = accept.encode() else {
            return refuse(writer, ServiceError::Internal).await;
        };
        if writer.frame(FrameKind::Accept, &accept).await.is_err() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
        }
        // Only `cancel` or the end of the caller's stream can come from the caller now.
        let caller_gone = async {
            while let Ok(Some(frame)) = reader.next().await {
                if frame.kind() == FrameKind::Cancel {
                    break;
                }
            }
        };
        tokio::pin!(caller_gone);
        let mut changes = self.subscribe();
        let mut sent = 0_u64;
        loop {
            changes.mark_unchanged();
            // The task is read before its events, so an ended task has no event left after
            // the ones read next.
            let record = match self.visible_task(viewer, task).await {
                Ok(record) => record,
                Err(error) => return finish(writer, Err(error)).await.with_bytes(0, sent),
            };
            let events = self
                .store(move |repository| repository.agent_task_events(task, (after, EVENT_BATCH)))
                .await;
            let Ok(events) = events else {
                return finish(writer, Err(ServiceError::Unavailable)).await;
            };
            let read = events.len();
            for event in events {
                let Ok(bytes) = event.to_bytes() else {
                    return finish(writer, Err(ServiceError::Internal)).await;
                };
                if writer.data(Channel::Output, &bytes).await.is_err() {
                    return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
                }
                sent += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                after = event.seq;
            }
            if read > 0 {
                continue;
            }
            if record.summary.state.is_terminal() {
                let ended = finish(writer, Ok(AgentReply::Summary(record.summary))).await;
                return ended.with_bytes(0, sent);
            }
            if writer.flush().await.is_err() {
                return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
            }
            tokio::select! {
                woke = changes.changed() => if woke.is_err() {
                    return finish(writer, Err(ServiceError::Unavailable)).await;
                },
                () = stop.cancelled() => {
                    return finish(writer, Err(ServiceError::Unavailable)).await;
                }
                () = &mut caller_gone => {
                    let _ended = finish(writer, Err(ServiceError::Cancelled)).await;
                    return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled))
                        .with_bytes(0, sent);
                }
            }
        }
    }
}
