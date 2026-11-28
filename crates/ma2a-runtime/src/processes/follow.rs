//! One follower per non-terminal Process: it watches the authenticated supervisor and
//! records every report, and decides the Process from its status record once the
//! supervisor is provably gone.

use std::time::Duration;

use ma2a_core::{
    EndpointId,
    service::{FrameKind, ProcessId, ProcessReason, ProcessState, ServiceError},
};
use ma2a_services::{
    FrameReader,
    supervisor::{Channel, Credentials, SupervisorRequest, SupervisorStatus},
};
use ma2a_store::ProcessRecord;

use super::{
    ProcessManager, credentials,
    go::{AbortCause, GoDispatch},
};

/// Outcome of settling a `starting` Process abort.
pub(super) enum AbortOutcome {
    Aborted,
    StillAuthorizedActive,
}

/// Longest pause between attempts to reach a supervisor that answered too slowly.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// How following one supervisor connection ended.
enum Followed {
    /// The supervisor reported its final status, which is recorded.
    Final,
    /// The connection broke before a final status; reach the supervisor again.
    Broken,
}

/// Ownership of a Process's follower slot held by `bring_up` until handed off to a
/// background follower task or released on completion.
pub(super) struct FollowGuard {
    manager: ProcessManager,
    id: ProcessId,
    armed: bool,
}

impl Drop for FollowGuard {
    fn drop(&mut self) {
        if self.armed {
            self.manager.coordination().followed.remove(&self.id);
        }
    }
}

impl ProcessManager {
    /// Claims the follower slot of `id` for initial bring-up unless a follower already owns it.
    pub(super) fn claim_follow(&self, id: ProcessId) -> Option<FollowGuard> {
        self.coordination()
            .followed
            .insert(id)
            .then(|| FollowGuard {
                manager: self.clone(),
                id,
                armed: true,
            })
    }

    /// Transfers an owned bring-up follower slot directly to the background follower task.
    pub(super) fn handoff_follow(&self, guard: Option<FollowGuard>, watching: Option<Channel>) {
        if let Some(mut guard) = guard {
            guard.armed = false;
            let manager = self.clone();
            let id = guard.id;
            self.0.tasks.spawn(async move {
                let shutdown = manager.0.shutdown.clone();
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = manager.follower(id, watching) => {}
                }
                manager.coordination().followed.remove(&id);
            });
        }
    }

    /// Starts the follower of one Process unless one already runs; `watching` is a proven
    /// connection on which `watch` was already accepted.
    pub(super) fn follow(&self, id: ProcessId, watching: Option<Channel>) {
        self.handoff_follow(self.claim_follow(id), watching);
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
            let channel = address.connect(credentials).await?;
            if let Some(cause) = self.aborted_starting_cause(record.id) {
                return self
                    .settle_starting_abort(((record.id, record.creator), cause), channel)
                    .await
                    .and_then(|outcome| match outcome {
                        AbortOutcome::Aborted => Ok(None),
                        AbortOutcome::StillAuthorizedActive => Err(ServiceError::Unavailable),
                    });
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
        ((id, creator), cause): ((ProcessId, EndpointId), AbortCause),
        channel: Channel,
    ) -> Result<AbortOutcome, ServiceError> {
        match request(channel, SupervisorRequest::Abort).await {
            Ok(reported) => self
                .apply(id, reported)
                .await
                .map(|_| AbortOutcome::Aborted),
            Err(ServiceError::ProcessActive) => {
                if cause == AbortCause::Expired {
                    match self.authorize_remote(creator).await {
                        Ok(()) => {
                            self.clear_expired_abort(id);
                            return Ok(AbortOutcome::StillAuthorizedActive);
                        }
                        Err(ServiceError::Unauthorized) => {
                            self.record_aborted_starting(id, AbortCause::Unauthorized);
                        }
                        Err(error) => return Err(error),
                    }
                }
                // A previous boot already sent `go` and authorization is withdrawn; terminate.
                self.terminate(
                    (None, id),
                    (
                        super::maintain::WITHDRAWN_GRACE_MS,
                        ProcessReason::Unauthorized,
                    ),
                )
                .await
                .map(|_| AbortOutcome::Aborted)
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
