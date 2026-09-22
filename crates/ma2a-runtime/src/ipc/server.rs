use std::sync::Arc;

use tokio::{
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{RuntimeHandle, current_user::CurrentUserRuntime};

use super::{
    CONNECTION_LIMIT, IpcError, IpcPaths, LIFECYCLE_RESERVE,
    lifecycle::{DaemonIdentity, RuntimeBoot},
    platform,
};

mod dispatch;
mod execute;

use dispatch::handle_connection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
/// Reason the local server stopped accepting connections.
pub enum ServerExit {
    /// The parent cancellation token was cancelled.
    Cancelled,
    /// A client completed the graceful shutdown command.
    ShutdownRequested,
    /// The Runtime actor stopped, so no command can be answered any more.
    ///
    /// Accepting connections after this point produces a daemon that completes a
    /// handshake and then closes every request without a response, which reads to
    /// a caller as a corrupt transport rather than a stopped Runtime.
    RuntimeStopped,
}

#[derive(Debug)]
/// Current-user local server backed by one Runtime handle.
pub struct LocalApiServer {
    listener: platform::PlatformListener,
    paths: IpcPaths,
    handle: RuntimeHandle,
    control: CurrentUserRuntime,
    replay: Arc<Mutex<()>>,
    web: Option<crate::web::WebLifecycle>,
    identity: Arc<DaemonIdentity>,
    permits: Arc<Semaphore>,
    reserve: Arc<Semaphore>,
}

impl LocalApiServer {
    /// Binds the platform endpoint after validating its private location.
    ///
    /// `identity` is answered verbatim to every lifecycle call, so it must
    /// describe the launch that is about to serve and must not change afterwards.
    ///
    /// # Errors
    /// Returns a platform transport or authorization error when binding fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "the server owns the endpoint, the Runtime, the control plane and its own identity as four independent things"
    )]
    pub fn bind(
        paths: IpcPaths,
        handle: RuntimeHandle,
        control: CurrentUserRuntime,
        identity: DaemonIdentity,
    ) -> Result<Self, IpcError> {
        let listener = platform::bind(&paths)?;
        Ok(Self {
            listener,
            paths,
            handle,
            control,
            replay: Arc::new(Mutex::new(())),
            web: None,
            identity: Arc::new(identity),
            permits: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
            reserve: Arc::new(Semaphore::new(LIFECYCLE_RESERVE)),
        })
    }

    /// Binds the endpoint and adopts the running Runtime's own identity.
    ///
    /// The identity is read once, here, while the Runtime is certain to answer.
    /// From then on the lifecycle plane holds it as data, which is what lets it
    /// keep answering after the Runtime cannot.
    ///
    /// # Errors
    /// Returns a Runtime error when the actor cannot report its boot, or a
    /// platform transport or authorization error when binding fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "the same four independent things, with the launch marker in place of a built identity"
    )]
    pub async fn bind_for_launch(
        paths: IpcPaths,
        handle: RuntimeHandle,
        control: CurrentUserRuntime,
        launch_nonce: Option<String>,
    ) -> Result<Self, IpcError> {
        let status = handle.status().await?;
        let identity = DaemonIdentity::of_current_process(
            launch_nonce,
            RuntimeBoot::new(status.boot_id(), status.endpoint_id()),
        );
        Self::bind(paths, handle, control, identity)
    }

    /// Returns the connection slots business traffic competes for.
    #[cfg(test)]
    pub(super) fn business_slots(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }

    /// Returns the lock every business command must take before it can execute.
    #[cfg(test)]
    pub(super) fn business_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.replay)
    }

    /// Associates the explicitly controlled daemon Web UI service.
    #[must_use]
    pub fn with_web_lifecycle(mut self, web: crate::web::WebLifecycle) -> Self {
        self.web = Some(web);
        self
    }

    /// Serves bounded concurrent connections until cancellation or graceful shutdown.
    ///
    /// # Errors
    /// Returns transport, Runtime, framing, or connection task failures.
    pub async fn serve(self, cancellation: CancellationToken) -> Result<ServerExit, IpcError> {
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);
        let mut tasks = JoinSet::new();
        let exit = loop {
            tokio::select! {
                () = cancellation.cancelled() => break ServerExit::Cancelled,
                shutdown = shutdown_receiver.recv() => {
                    if shutdown.is_some() { break ServerExit::ShutdownRequested; }
                }
                () = self.handle.stopped() => break ServerExit::RuntimeStopped,
                accepted = platform::accept(&self.listener) => {
                    let stream = accepted?;
                    let Some(admission) = admit(&self.permits, &self.reserve)? else { continue };
                    let context = ConnectionContext {
                        paths: self.paths.clone(),
                        handle: self.handle.clone(),
                        control: self.control.clone(),
                        replay: Arc::clone(&self.replay),
                        shutdown_sender: shutdown_sender.clone(),
                        web: self.web.clone(),
                        identity: Arc::clone(&self.identity),
                    };
                    tasks.spawn(async move {
                        let lifecycle_only = admission.lifecycle_only;
                        let _permit = admission.permit;
                        handle_connection(stream, context, lifecycle_only).await
                    });
                }
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(result) = joined { let _connection_result = result?; }
                }
            }
        };
        cancellation.cancel();
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                return Err(error.into());
            }
        }
        Ok(exit)
    }
}

struct Admission {
    permit: OwnedSemaphorePermit,
    lifecycle_only: bool,
}

/// Admits a connection, falling back to the reserve kept for lifecycle calls.
///
/// `Ok(None)` means every slot including the reserve is taken, which is the only
/// case where a connection is dropped unanswered.
fn admit(
    permits: &Arc<Semaphore>,
    reserve: &Arc<Semaphore>,
) -> Result<Option<Admission>, IpcError> {
    match Arc::clone(permits).try_acquire_owned() {
        Ok(permit) => Ok(Some(Admission {
            permit,
            lifecycle_only: false,
        })),
        Err(TryAcquireError::NoPermits) => match Arc::clone(reserve).try_acquire_owned() {
            Ok(permit) => Ok(Some(Admission {
                permit,
                lifecycle_only: true,
            })),
            Err(TryAcquireError::NoPermits) => Ok(None),
            Err(TryAcquireError::Closed) => Err(IpcError::InvalidFrame),
        },
        Err(TryAcquireError::Closed) => Err(IpcError::InvalidFrame),
    }
}

struct ConnectionContext {
    paths: IpcPaths,
    handle: RuntimeHandle,
    control: CurrentUserRuntime,
    replay: Arc<Mutex<()>>,
    shutdown_sender: mpsc::Sender<()>,
    web: Option<crate::web::WebLifecycle>,
    identity: Arc<DaemonIdentity>,
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
