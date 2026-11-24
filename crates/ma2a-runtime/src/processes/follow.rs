//! One follower per non-terminal Process: it watches the authenticated supervisor and
//! records every report, and decides the Process from its status record once the
//! supervisor is provably gone.

use std::time::Duration;

use ma2a_core::service::{FrameKind, ProcessId, ProcessState, ServiceError};
use ma2a_services::supervisor::{Channel, Credentials, SupervisorRequest, SupervisorStatus};

use super::{ProcessManager, credentials};

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
                Some(channel) => Ok(channel),
                None => self.watch(&credentials, record.state).await,
            };
            match channel {
                Ok(channel) => {
                    if matches!(self.relay_statuses(id, channel).await, Followed::Final) {
                        return;
                    }
                    // A broken watch is reopened promptly, but never in a tight loop.
                    backoff = pause(Duration::from_millis(250)).await;
                }
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

    /// Opens a proven `watch`; a Process still `starting` is sent `go` first, which a
    /// supervisor that already spawned its child answers without effect.
    pub(super) async fn watch(
        &self,
        credentials: &Credentials,
        state: ProcessState,
    ) -> Result<Channel, ServiceError> {
        let address = self.address(credentials.id());
        if state == ProcessState::Starting {
            let reported =
                request(address.connect(credentials).await?, SupervisorRequest::Go).await?;
            self.apply(credentials.id(), reported).await?;
        }
        super::start::open_watch(address.connect(credentials).await?).await
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
    let frame = answered.map_err(|_| ServiceError::Unavailable)??;
    match frame.kind() {
        FrameKind::Outcome => {
            SupervisorStatus::from_bytes(frame.payload()).map_err(|_| ServiceError::Unavailable)
        }
        FrameKind::Reject => Err(frame.rejection()?),
        _ => Err(ServiceError::Unavailable),
    }
}
