//! Ordered `go` dispatch against durable Process authorization.

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use ma2a_core::{
    EndpointId,
    service::{FrameKind, ProcessId, ServiceError},
};
use ma2a_services::supervisor::{Channel, SupervisorRequest, SupervisorStatus};

use super::{
    ProcessManager,
    follow::{ANSWER_MARGIN, read_supervisor_status, request},
};

/// Outcome of deciding and dispatching `go` on an authenticated supervisor channel.
pub(super) enum GoDispatch {
    /// `go` was sent and the supervisor reported this status.
    Reported(SupervisorStatus),
    /// Authorization was withdrawn before `go`; the supervisor was aborted and settled.
    Aborted,
    /// Authorization was withdrawn, and settling the abort failed transiently.
    AbortFailed(ServiceError),
    /// The authorization check or `go` transport failed transiently; retry without starting.
    Deferred(ServiceError),
}

struct GoDispatchGuard(Arc<(Mutex<bool>, Condvar)>);

impl Drop for GoDispatchGuard {
    fn drop(&mut self) {
        let (done, signal) = &*self.0;
        *done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        signal.notify_one();
    }
}

impl ProcessManager {
    /// Checks remote authorization (if remote), then orders `go` dispatch with authorization
    /// changes on the Store so a completed withdrawal aborts instead of starting the child.
    pub(super) async fn dispatch_go(
        &self,
        (id, creator): (ProcessId, EndpointId),
        channel: Channel,
    ) -> GoDispatch {
        if creator == self.0.local {
            return match request(channel, SupervisorRequest::Go).await {
                Ok(reported) => GoDispatch::Reported(reported),
                Err(error) => GoDispatch::Deferred(error),
            };
        }
        #[cfg(test)]
        self.wait_before_go_gate().await;
        match self.authorize_remote(creator).await {
            Ok(()) => {}
            Err(ServiceError::Unauthorized) => return self.abort_unauthorized(id, channel).await,
            Err(error) => return GoDispatch::Deferred(error),
        }
        #[cfg(test)]
        self.wait_after_authorize_gate().await;
        self.authorize_and_send_go((id, creator), channel).await
    }

    async fn abort_unauthorized(&self, id: ProcessId, channel: Channel) -> GoDispatch {
        match self.settle_unauthorized_starting(id, channel).await {
            Ok(()) => GoDispatch::Aborted,
            Err(error) => GoDispatch::AbortFailed(error),
        }
    }

    async fn authorize_and_send_go(
        &self,
        (id, creator): (ProcessId, EndpointId),
        (mut reader, mut writer): Channel,
    ) -> GoDispatch {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let guard = GoDispatchGuard(Arc::clone(&signal));
        if let Err(error) = self
            .store()
            .authorize_process_go((self.0.local, creator), signal)
            .await
        {
            drop(guard);
            return if error == ServiceError::Unauthorized {
                self.abort_unauthorized(id, (reader, writer)).await
            } else {
                GoDispatch::Deferred(error)
            };
        }
        let sent = tokio::time::timeout(super::host::FRAME_GRACE, async {
            writer
                .frame(FrameKind::Open, &SupervisorRequest::Go.to_bytes())
                .await?;
            writer.flush().await
        })
        .await;
        // Release the Store thread as soon as `go` is flushed (or failed/timed out).
        drop(guard);
        match sent {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return GoDispatch::Deferred(error),
            Err(_) => return GoDispatch::Deferred(ServiceError::Unavailable),
        }
        match read_supervisor_status(&mut reader, ANSWER_MARGIN).await {
            Ok(reported) => GoDispatch::Reported(reported),
            Err(error) => GoDispatch::Deferred(error),
        }
    }

    #[cfg(test)]
    async fn wait_gate(gate: &super::TestGate) {
        let gate = gate.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some((entered, release)) = gate {
            let _entered = entered.send(());
            let _released = release.await;
        }
    }

    #[cfg(test)]
    async fn wait_before_go_gate(&self) {
        Self::wait_gate(&self.0.before_go_gate).await;
    }

    #[cfg(test)]
    async fn wait_after_authorize_gate(&self) {
        Self::wait_gate(&self.0.after_authorize_gate).await;
    }
}
