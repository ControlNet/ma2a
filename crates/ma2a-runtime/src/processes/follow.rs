//! One follower per non-terminal Process: it watches the authenticated supervisor and
//! records every report, and decides the Process from its status record once the
//! supervisor is provably gone.

use std::time::Duration;

use ma2a_core::service::{FrameKind, ProcessId, ProcessReason, ProcessState, ServiceError};
use ma2a_services::{
    FrameReader,
    supervisor::{Channel, Credentials, SupervisorRequest, SupervisorStatus},
};
use ma2a_store::ProcessRecord;

use super::{ProcessManager, credentials, go::GoDispatch};

/// Longest pause between attempts to reach a supervisor that answered too slowly.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// How following one supervisor connection ended.
enum Followed {
    /// The supervisor reported its final status, which is recorded.
    Final,
    /// The connection broke before a final status; reach the supervisor again.
    Broken,
}

impl ProcessManager {
    /// Starts the follower of one Process unless one already runs; `watching` is a proven
    /// connection on which `watch` was already accepted.
    pub(super) fn follow(&self, id: ProcessId, watching: Option<Channel>) {
        if !self.coordination().followed.insert(id) {
            return;
        }
        let manager = self.clone();
        self.0.tasks.spawn(async move {
            let shutdown = manager.0.shutdown.clone();
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = manager.follower(id, watching) => {}
            }
            manager.coordination().followed.remove(&id);
        });
    }

    /// Follows until the Process is terminal or gone; a transient Store or supervisor
    /// failure is retried with backoff, never taken as the end of following.
    async fn follower(&self, id: ProcessId, mut watching: Option<Channel>) {
        let mut backoff = Duration::from_millis(250);
        loop {
            let record = match self.record(id).await {
                Ok(Some(record)) => record,
                Ok(None) => return,
                Err(_) => {
                    backoff = pause(backoff).await;
                    continue;
                }
            };
            let Some(credentials) = credentials(&record).filter(|_| !record.state.is_terminal())
            else {
                return;
            };
            let channel = match watching.take() {
                Some(channel) => Ok(Some(channel)),
                None => self.watch_record(&record, &credentials).await,
            };
            match channel {
                Ok(Some(channel)) => {
                    if matches!(self.relay_statuses(id, channel).await, Followed::Final) {
                        return;
                    }
                    // A broken watch is reopened promptly, but never in a tight loop.
                    backoff = pause(Duration::from_millis(250)).await;
                }
                Ok(None) => {}
                // Nothing listens, or something that cannot prove the secret does.
                Err(ServiceError::Unreachable | ServiceError::Unauthorized) => {
                    if self.conclude(&record).await.is_ok() {
                        return;
                    }
                    backoff = pause(backoff).await;
                }
                // A slow or busy supervisor is never mistaken for a lost one.
                Err(_) => backoff = pause(backoff).await,
            }
        }
    }

    async fn watch_record(
        &self,
        record: &ProcessRecord,
        credentials: &Credentials,
    ) -> Result<Option<Channel>, ServiceError> {
        let address = self.address(credentials.id());
        if record.state == ProcessState::Starting {
            let abort_cause = self
                .coordination()
                .aborted_starting
                .get(&record.id)
                .copied();
            let channel = address.connect(credentials).await?;
            if abort_cause.is_some() {
                return self
                    .settle_starting_abort(record.id, channel)
                    .await
                    .map(|()| None);
            }
            match self
                .dispatch_go((credentials, record.creator), channel)
                .await
            {
                GoDispatch::Reported(reported) => {
                    self.apply(credentials.id(), reported).await?;
                }
                GoDispatch::Aborted => return Ok(None),
                GoDispatch::AbortFailed(error) | GoDispatch::Deferred(error) => return Err(error),
            }
        }
        super::start::open_watch(address.connect(credentials).await?)
            .await
            .map(Some)
    }

    pub(super) async fn settle_starting_abort(
        &self,
        id: ProcessId,
        channel: Channel,
    ) -> Result<(), ServiceError> {
        match request(channel, SupervisorRequest::Abort).await {
            Ok(reported) => self.apply(id, reported).await.map(|_| ()),
            Err(ServiceError::ProcessActive) => {
                // A previous boot already sent `go` before crashing; terminate the running child.
                self.terminate(
                    (None, id),
                    (
                        super::maintain::WITHDRAWN_GRACE_MS,
                        ProcessReason::Unauthorized,
                    ),
                )
                .await
                .map(|_| ())
            }
            // Do not pass supervisor-side `Unauthorized` back to `follower`'s proof-loss arm.
            Err(ServiceError::Unauthorized) => Err(ServiceError::Unavailable),
            Err(error) => Err(error),
        }
    }

    /// Opens a proven `watch`; used by tests on already-running Processes.
    #[cfg(all(test, unix))]
    pub(super) async fn watch(
        &self,
        credentials: &Credentials,
        state: ProcessState,
    ) -> Result<Channel, ServiceError> {
        if state == ProcessState::Running {
            let address = self.address(credentials.id());
            return super::start::open_watch(address.connect(credentials).await?).await;
        }
        let record = self
            .record(credentials.id())
            .await?
            .ok_or(ServiceError::UnknownProcess)?;
        self.watch_record(&record, credentials)
            .await?
            .ok_or(ServiceError::NotRunning)
    }

    async fn relay_statuses(&self, id: ProcessId, (mut reader, _writer): Channel) -> Followed {
        loop {
            let Ok(frame) = reader.expect().await else {
                return Followed::Broken;
            };
            let Ok(reported) = SupervisorStatus::from_bytes(frame.payload()) else {
                return Followed::Broken;
            };
            match (frame.kind(), self.apply(id, reported).await) {
                (FrameKind::Outcome, Ok(_)) => return Followed::Final,
                (FrameKind::Control, Ok(_)) => {}
                _ => return Followed::Broken,
            }
        }
    }
}

/// Sleeps `backoff` and returns the next, longer one.
async fn pause(backoff: Duration) -> Duration {
    tokio::time::sleep(backoff).await;
    (backoff * 2).min(MAX_BACKOFF)
}

/// Longest wait for a supervisor's answer beyond the time its request may take by design.
pub(super) const ANSWER_MARGIN: Duration = Duration::from_secs(10);

pub(super) async fn read_supervisor_status<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    deadline: Duration,
) -> Result<SupervisorStatus, ServiceError> {
    let frame = tokio::time::timeout(deadline, reader.expect())
        .await
        .map_err(|_| ServiceError::Unavailable)??;
    decode_supervisor_status(&frame)
}

fn decode_supervisor_status(
    frame: &ma2a_services::Frame,
) -> Result<SupervisorStatus, ServiceError> {
    match frame.kind() {
        FrameKind::Outcome => {
            SupervisorStatus::from_bytes(frame.payload()).map_err(|_| ServiceError::Unavailable)
        }
        FrameKind::Reject => Err(frame.rejection()?),
        _ => Err(ServiceError::Unavailable),
    }
}

/// Sends one request on a proven connection and returns the status in its `outcome`.
///
/// A supervisor that does not answer in time is `Unavailable`, never lost: `terminate`
/// may take its grace period plus the margin, which a child stuck in the kernel exceeds.
pub(super) async fn request(
    (mut reader, mut writer): Channel,
    request: SupervisorRequest,
) -> Result<SupervisorStatus, ServiceError> {
    let deadline = match request {
        SupervisorRequest::Terminate(grace_ms) => {
            Duration::from_millis(u64::from(grace_ms)) + ANSWER_MARGIN
        }
        _ => ANSWER_MARGIN,
    };
    let answered = tokio::time::timeout(deadline, async {
        writer.frame(FrameKind::Open, &request.to_bytes()).await?;
        reader.expect().await
    })
    .await;
    decode_supervisor_status(&answered.map_err(|_| ServiceError::Unavailable)??)
}
