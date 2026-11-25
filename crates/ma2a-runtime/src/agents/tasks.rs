//! Task operations shared by remote callers and the local operator.
//!
//! A remote caller sees only the tasks it created; the local operator (`None`) sees every
//! task. Every operation names its Agent, and a task of another Agent does not exist for it.

use std::time::Duration;

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentEvent, AgentId, AgentMessage, AgentSubmit, AgentTaskDetails, AgentTaskId,
        AgentTaskReason, MessageRole, ServiceError,
    },
};
use ma2a_store::{
    AgentTaskChange, AgentTaskCreation, AgentTaskFilter, AgentTaskRecord, NewAgentTask,
};

use tokio_util::sync::CancellationToken;

use super::{AgentManager, convert::valid_data, fault, visible};

/// Who asks (`None` is the local operator) and the Agent the operation names, if any.
pub(crate) type Viewer<'a> = (Option<EndpointId>, Option<&'a AgentId>);

/// What a creator's message did to its live task.
#[derive(Debug)]
pub(crate) enum Sent {
    /// The message reached the task; the task as it now is, and the revision of that commit.
    Delivered(Box<AgentTaskRecord>, u64),
    /// The task already held as many durable events as it may, so the message failed it
    /// instead, committed at this revision: the task ended because of this send.
    FailedTask(u64),
}

/// Fingerprints a canonical submission; a retry must send exactly the same bytes.
fn fingerprint(submit: &AgentSubmit) -> Result<[u8; 32], ServiceError> {
    let bytes = submit.to_bytes().map_err(|_| ServiceError::InvalidInput)?;
    Ok(*blake3::Hasher::new_derive_key("ma2a agent submit v1")
        .update(&bytes)
        .finalize()
        .as_bytes())
}

/// Validates a creator's message: role `user`, and every `data` part one JSON value.
fn user_message(message: &AgentMessage) -> Result<(), ServiceError> {
    if message.role() == MessageRole::User && valid_data(message) {
        Ok(())
    } else {
        Err(ServiceError::InvalidInput)
    }
}

impl AgentManager {
    /// Creates one task under `request_id`, or answers the task that request already made;
    /// `true` when it already existed. The revision is the one the task was read at in the
    /// creating transaction.
    pub(crate) async fn submit(
        &self,
        binding: (EndpointId, RequestId),
        submit: &AgentSubmit,
    ) -> Result<((AgentTaskRecord, bool), u64), ServiceError> {
        user_message(submit.message())?;
        let task = NewAgentTask {
            id: AgentTaskId::random().map_err(|_| ServiceError::Internal)?,
            agent: submit.agent().clone(),
            binding,
            fingerprint: fingerprint(submit)?,
            from_agent: submit.from_agent().cloned(),
            input: submit.message().clone(),
            created_ms: self.now_ms(),
        };
        let (created, revision) = self
            .commit_at(move |repository| repository.create_agent_task(&task))
            .await?;
        match created {
            AgentTaskCreation::Created(record) => {
                fault("after_task_created");
                self.assign(submit.agent()).await;
                Ok(((*record, false), revision))
            }
            AgentTaskCreation::Existing(record) => Ok(((*record, true), revision)),
            AgentTaskCreation::Conflict => Err(ServiceError::Conflict),
            AgentTaskCreation::UnknownAgent => Err(ServiceError::UnknownAgent),
            AgentTaskCreation::Unavailable => Err(ServiceError::AgentUnavailable),
            AgentTaskCreation::LimitExceeded => Err(ServiceError::ConcurrencyExceeded),
        }
    }

    /// Returns one task the viewer may see.
    pub(crate) async fn visible_task(
        &self,
        viewer: Viewer<'_>,
        task: AgentTaskId,
    ) -> Result<AgentTaskRecord, ServiceError> {
        visible(self.task(task).await?, viewer)
    }

    /// Returns one task with its artifacts, as of one moment.
    pub(crate) async fn details(
        &self,
        viewer: Viewer<'_>,
        task: AgentTaskId,
    ) -> Result<AgentTaskDetails, ServiceError> {
        Ok(self.details_at(viewer, task).await?.1)
    }

    /// Returns one task with its artifacts, both from one read, with the revision of that read.
    pub(crate) async fn details_at(
        &self,
        viewer: Viewer<'_>,
        task: AgentTaskId,
    ) -> Result<(u64, AgentTaskDetails), ServiceError> {
        let read = self
            .store(move |repository| repository.agent_task_details(task))
            .await?;
        let (revision, read) = (read.revision(), read.into_value());
        let (record, artifacts) = read.ok_or(ServiceError::UnknownTask)?;
        let record = visible(Some(record), viewer)?;
        Ok((revision, record.details(artifacts)))
    }

    /// Delivers the creator's next message to a live task.
    pub(crate) async fn send(
        &self,
        viewer: Viewer<'_>,
        (task, message): (AgentTaskId, AgentMessage),
    ) -> Result<Sent, ServiceError> {
        user_message(&message)?;
        self.visible_task(viewer, task).await?;
        let at_ms = self.now_ms();
        match self
            .commit_at(move |repository| repository.send_agent_message(task, (&message, at_ms)))
            .await?
        {
            (AgentTaskChange::Changed(record), revision) => Ok(Sent::Delivered(record, revision)),
            (AgentTaskChange::Failed(_), revision) => {
                self.task_ended(task);
                Ok(Sent::FailedTask(revision))
            }
            (AgentTaskChange::Ended(_), _) => Err(ServiceError::TaskEnded),
            (AgentTaskChange::Refused, _) => Err(ServiceError::ConcurrencyExceeded),
            (AgentTaskChange::Missing, _) => Err(ServiceError::UnknownTask),
        }
    }

    /// Cancels a task and answers once it ended: at once when queued, otherwise when its
    /// Agent confirms or the grace period passes. When `stop` fires first, the request stands
    /// but its end is unknown.
    pub(crate) async fn cancel(
        &self,
        viewer: Viewer<'_>,
        (task, reason, stop): (AgentTaskId, AgentTaskReason, &CancellationToken),
    ) -> Result<AgentTaskRecord, ServiceError> {
        let (record, _) = self.request_cancel(viewer, (task, reason)).await?;
        if record.summary.state.is_terminal() {
            return Ok(record);
        }
        // The request is committed from here on, so any failure to see the end leaves the
        // outcome unknown, never refused.
        tokio::select! {
            biased;
            () = stop.cancelled() => Err(ServiceError::OutcomeUnknown),
            ended = self.wait_ended(task, Duration::from_secs(60)) => ended
                .ok()
                .flatten()
                .filter(|record| record.summary.state.is_terminal())
                .ok_or(ServiceError::OutcomeUnknown),
        }
    }

    /// Requests a task's cancellation and answers the task as it then is, with the revision
    /// of that commit: ended when it was queued, otherwise still live until its Agent confirms
    /// or the grace period passes.
    pub(crate) async fn request_cancel(
        &self,
        viewer: Viewer<'_>,
        (task, reason): (AgentTaskId, AgentTaskReason),
    ) -> Result<(AgentTaskRecord, u64), ServiceError> {
        self.visible_task(viewer, task).await?;
        let at_ms = self.now_ms();
        let (changed, revision) = self
            .commit_at(move |repository| repository.request_agent_cancel(task, (reason, at_ms)))
            .await?;
        match changed {
            AgentTaskChange::Changed(record)
            | AgentTaskChange::Ended(record)
            | AgentTaskChange::Failed(record) => {
                if record.summary.state.is_terminal() {
                    self.task_ended(task);
                }
                Ok((*record, revision))
            }
            AgentTaskChange::Refused => Ok((self.visible_task(viewer, task).await?, revision)),
            AgentTaskChange::Missing => Err(ServiceError::UnknownTask),
        }
    }

    /// Waits until a task ended or `timeout` passed, and returns it as it then is.
    pub(crate) async fn wait_ended(
        &self,
        task: AgentTaskId,
        timeout: Duration,
    ) -> Result<Option<AgentTaskRecord>, ServiceError> {
        let mut changes = self.subscribe();
        let waited = tokio::time::timeout(timeout, async {
            loop {
                changes.mark_unchanged();
                match self.task(task).await? {
                    Some(record) if !record.summary.state.is_terminal() => {}
                    other => return Ok(other),
                }
                if changes.changed().await.is_err() {
                    return Err(ServiceError::Unavailable);
                }
            }
        })
        .await;
        match waited {
            Ok(ended) => ended,
            Err(_) => self.task(task).await,
        }
    }

    /// Lists the viewer's tasks of one Agent, or every Agent, in identifier order.
    pub(crate) async fn tasks(
        &self,
        (creator, agent): (Option<EndpointId>, Option<AgentId>),
        (after, limit): (Option<AgentTaskId>, u32),
    ) -> Result<(u64, Vec<AgentTaskRecord>), ServiceError> {
        let filter = AgentTaskFilter {
            creator,
            agent,
            after,
            limit,
        };
        let listed = self
            .store(move |repository| repository.agent_tasks(&filter))
            .await?;
        Ok((listed.revision(), listed.into_value()))
    }

    /// Returns at most `limit` retained events of a visible task after `after`, from one read,
    /// with the revision of that read.
    pub(crate) async fn events(
        &self,
        viewer: Viewer<'_>,
        (task, after, limit): (AgentTaskId, u32, u32),
    ) -> Result<(u64, Vec<AgentEvent>), ServiceError> {
        let read = self
            .store(move |repository| repository.agent_task_events_at(task, (after, limit)))
            .await?;
        let (revision, read) = (read.revision(), read.into_value());
        let (record, events) = read.ok_or(ServiceError::UnknownTask)?;
        visible(Some(record), viewer)?;
        Ok((revision, events))
    }
}
