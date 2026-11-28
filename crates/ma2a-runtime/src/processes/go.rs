//! Ordered `go` dispatch against durable Process authorization.

use std::{pin::Pin, task::Poll};

use ma2a_core::{
    EndpointId,
    service::{FrameHeader, FrameKind, ProcessId, ServiceError},
};
use ma2a_services::supervisor::{Channel, Credentials, SupervisorRequest, SupervisorStatus};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};

use super::{
    ProcessManager,
    follow::{ANSWER_MARGIN, AbortOutcome, read_supervisor_status, request},
};
use crate::store::GoDispatchLease;

/// Why a `starting` Process must be aborted instead of sending `go` on retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AbortCause {
    Expired,
    Unauthorized,
}

/// Outcome of deciding and dispatching `go` on an authenticated supervisor channel.
pub(super) enum GoDispatch {
    /// `go` was sent and the supervisor reported this status.
    Reported(SupervisorStatus),
    /// Authorization was withdrawn or the dispatch lease expired before `go`; the supervisor
    /// was aborted and settled.
    Aborted,
    /// Authorization was withdrawn or the lease expired, and settling the abort failed transiently.
    AbortFailed(ServiceError),
    /// The authorization check or `go` transport failed transiently; retry without starting.
    Deferred(ServiceError),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WriteGoOutcome {
    Sent,
    Expired { wrote_any: bool },
    Failed(ServiceError),
}

#[cfg(test)]
pub(super) type TestGate = std::sync::Mutex<
    Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
>;

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct GoTestGates {
    pub(super) before_go: TestGate,
    pub(super) after_authorize: TestGate,
    pub(super) after_final_authorize: TestGate,
}

struct GoDispatchGuard(GoDispatchLease);

impl Drop for GoDispatchGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}

pub(super) fn go_frame() -> Result<Vec<u8>, ServiceError> {
    let payload = SupervisorRequest::Go.to_bytes();
    let header = FrameHeader::new(FrameKind::Open, payload.len())
        .and_then(FrameHeader::encode)
        .map_err(|_| ServiceError::InvalidInput)?;
    let mut frame = Vec::with_capacity(header.len() + payload.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub(super) async fn write_go_with_lease<W: AsyncWrite + Unpin>(
    stream: &mut W,
    frame: &[u8],
    lease: &GoDispatchLease,
) -> WriteGoOutcome {
    let mut written = 0_usize;
    let outcome = {
        let write = std::future::poll_fn(|cx| {
            lease
                .try_while_waiting(|| {
                    while let Some(rest) = frame.get(written..).filter(|rest| !rest.is_empty()) {
                        match Pin::new(&mut *stream).poll_write(cx, rest) {
                            Poll::Ready(Ok(0) | Err(_)) => {
                                return (
                                    Poll::Ready(WriteGoOutcome::Failed(ServiceError::Cancelled)),
                                    true,
                                );
                            }
                            Poll::Ready(Ok(n)) => written += n,
                            Poll::Pending => return (Poll::Pending, false),
                        }
                    }
                    (Poll::Ready(WriteGoOutcome::Sent), true)
                })
                .unwrap_or(Poll::Ready(WriteGoOutcome::Expired { wrote_any: false }))
        });
        tokio::select! {
            biased;
            () = lease.expired() => WriteGoOutcome::Expired { wrote_any: false },
            outcome = write => outcome,
        }
    };
    match outcome {
        WriteGoOutcome::Sent => match stream.flush().await {
            Ok(()) => WriteGoOutcome::Sent,
            Err(_) => WriteGoOutcome::Failed(ServiceError::Cancelled),
        },
        WriteGoOutcome::Expired { .. } => WriteGoOutcome::Expired {
            wrote_any: written > 0,
        },
        WriteGoOutcome::Failed(error) => WriteGoOutcome::Failed(error),
    }
}

impl ProcessManager {
    pub(super) fn record_aborted_starting(&self, id: ProcessId, cause: AbortCause) {
        let mut coordination = self.coordination();
        coordination
            .aborted_starting
            .entry(id)
            .and_modify(|existing| {
                if cause == AbortCause::Unauthorized {
                    *existing = AbortCause::Unauthorized;
                }
            })
            .or_insert(cause);
    }

    pub(super) fn clear_expired_abort(&self, id: ProcessId) {
        let mut coordination = self.coordination();
        if coordination.aborted_starting.get(&id) == Some(&AbortCause::Expired) {
            coordination.aborted_starting.remove(&id);
        }
    }

    pub(super) fn aborted_starting_cause(&self, id: ProcessId) -> Option<AbortCause> {
        self.coordination().aborted_starting.get(&id).copied()
    }

    pub(super) fn clear_process_coordination(&self, id: ProcessId) {
        let mut coordination = self.coordination();
        coordination.terminating.remove(&id);
        coordination.aborted_starting.remove(&id);
    }

    /// Checks remote authorization (if remote), then orders `go` dispatch with authorization
    /// changes on the Store so a completed withdrawal aborts instead of starting the child.
    pub(super) async fn dispatch_go(
        &self,
        (credentials, creator): (&Credentials, EndpointId),
        channel: Channel,
    ) -> GoDispatch {
        if let Some(cause) = self.aborted_starting_cause(credentials.id()) {
            return self
                .abort_starting(((credentials.id(), creator), cause), channel)
                .await;
        }
        if creator == self.0.local {
            return match request(channel, SupervisorRequest::Go).await {
                Ok(reported) => GoDispatch::Reported(reported),
                Err(error) => GoDispatch::Deferred(error),
            };
        }
        #[cfg(test)]
        Self::wait_gate(&self.0.gates.before_go).await;
        match self.authorize_remote(creator).await {
            Ok(()) => {}
            Err(ServiceError::Unauthorized) => {
                return self
                    .abort_starting(
                        ((credentials.id(), creator), AbortCause::Unauthorized),
                        channel,
                    )
                    .await;
            }
            Err(error) => return GoDispatch::Deferred(error),
        }
        #[cfg(test)]
        Self::wait_gate(&self.0.gates.after_authorize).await;
        self.authorize_and_send_go((credentials, creator), channel)
            .await
    }

    async fn abort_starting(
        &self,
        ((id, creator), cause): ((ProcessId, EndpointId), AbortCause),
        channel: Channel,
    ) -> GoDispatch {
        self.record_aborted_starting(id, cause);
        match self
            .settle_starting_abort(((id, creator), cause), channel)
            .await
        {
            Ok(AbortOutcome::Aborted) => GoDispatch::Aborted,
            Ok(AbortOutcome::StillAuthorizedActive) => {
                GoDispatch::Deferred(ServiceError::Unavailable)
            }
            Err(error) => GoDispatch::AbortFailed(error),
        }
    }

    async fn authorize_and_send_go(
        &self,
        (credentials, creator): (&Credentials, EndpointId),
        (mut reader, mut writer): Channel,
    ) -> GoDispatch {
        let id = credentials.id();
        if let Some(cause) = self.aborted_starting_cause(id) {
            return self
                .abort_starting(((id, creator), cause), (reader, writer))
                .await;
        }
        let lease = GoDispatchLease::new();
        let guard = GoDispatchGuard(lease.clone());
        if let Err(error) = self
            .store()
            .authorize_process_go((self.0.local, creator), lease.clone())
            .await
        {
            drop(guard);
            return if error == ServiceError::Unauthorized {
                self.abort_starting(((id, creator), AbortCause::Unauthorized), (reader, writer))
                    .await
            } else {
                GoDispatch::Deferred(error)
            };
        }
        #[cfg(test)]
        Self::wait_gate(&self.0.gates.after_final_authorize).await;
        if let Some(cause) = self.aborted_starting_cause(id) {
            drop(guard);
            return self
                .abort_starting(((id, creator), cause), (reader, writer))
                .await;
        }
        let Ok(frame) = go_frame() else {
            drop(guard);
            return GoDispatch::Deferred(ServiceError::InvalidInput);
        };
        let sent = tokio::time::timeout(
            super::host::FRAME_GRACE,
            write_go_with_lease(writer.get_mut(), &frame, &lease),
        )
        .await;
        // Release the Store thread as soon as `go` is written (or failed/timed out).
        drop(guard);
        match sent {
            Ok(WriteGoOutcome::Sent) => {}
            Ok(WriteGoOutcome::Expired { .. }) => {
                drop((reader, writer));
                self.record_aborted_starting(id, AbortCause::Expired);
                return match self.address(id).connect(credentials).await {
                    Ok(channel) => {
                        self.abort_starting(((id, creator), AbortCause::Expired), channel)
                            .await
                    }
                    Err(error) => GoDispatch::AbortFailed(error),
                };
            }
            Ok(WriteGoOutcome::Failed(error)) => return GoDispatch::Deferred(error),
            Err(_) => return GoDispatch::Deferred(ServiceError::Unavailable),
        }
        match read_supervisor_status(&mut reader, ANSWER_MARGIN).await {
            Ok(reported) => GoDispatch::Reported(reported),
            Err(error) => GoDispatch::Deferred(error),
        }
    }

    #[cfg(test)]
    async fn wait_gate(gate: &TestGate) {
        let gate = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((entered, release)) = gate {
            let _entered = entered.send(());
            let _released = release.await;
        }
    }
}
