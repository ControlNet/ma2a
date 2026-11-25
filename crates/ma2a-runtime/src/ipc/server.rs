use std::sync::Arc;

use tokio::{
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{RuntimeHandle, current_user::CurrentUserRuntime};

use super::{
    ACCEPT_RESERVE, BUSINESS_CONNECTION_LIMIT, IpcError, IpcPaths, LIFECYCLE_RESERVE,
    lifecycle::{DaemonIdentity, RuntimeBoot},
    platform,
};

mod dispatch;
mod execute;
mod hosted;
mod mutation_outcome;
#[cfg(test)]
mod mutation_test_hooks;
mod stream;

use dispatch::handle_connection;

/// How long to wait after a failed accept, which could otherwise fail again at once.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

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
    #[cfg(test)]
    mutation_hooks: mutation_test_hooks::MutationTestHooks,
    permits: Arc<Semaphore>,
    accepts: Arc<Semaphore>,
    reserve: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    in_flight: crate::stream_plane::InFlight,
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
        // Agent tools reach other Endpoints through this same local API.
        if let Some(agents) = handle.agents() {
            agents.install_loopback(paths.clone());
        }
        Ok(Self {
            listener,
            paths,
            handle,
            control,
            replay: Arc::new(Mutex::new(())),
            web: None,
            identity: Arc::new(identity),
            #[cfg(test)]
            mutation_hooks: mutation_test_hooks::MutationTestHooks::default(),
            permits: Arc::new(Semaphore::new(BUSINESS_CONNECTION_LIMIT)),
            accepts: Arc::new(Semaphore::new(ACCEPT_RESERVE)),
            reserve: Arc::new(Semaphore::new(LIFECYCLE_RESERVE)),
            streams: Arc::new(Semaphore::new(crate::stream_plane::STREAM_CONNECTION_LIMIT)),
            in_flight: crate::stream_plane::InFlight::default(),
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
                    // A failed accept, such as one at the descriptor limit or a pipe client gone
                    // before it was accepted, ends one connection, never the daemon.
                    let Ok(stream) = accepted else {
                        tokio::select! {
                            () = cancellation.cancelled() => break ServerExit::Cancelled,
                            () = tokio::time::sleep(ACCEPT_BACKOFF) => continue,
                        }
                    };
                    let pools = [
                        (&self.permits, Class::Business),
                        (&self.accepts, Class::Accept),
                        (&self.reserve, Class::Lifecycle),
                    ];
                    let Some(admission) = admit(pools)? else { continue };
                    let context = ConnectionContext {
                        paths: self.paths.clone(),
                        handle: self.handle.clone(),
                        control: self.control.clone(),
                        replay: Arc::clone(&self.replay),
                        shutdown_sender: shutdown_sender.clone(),
                        web: self.web.clone(),
                        identity: Arc::clone(&self.identity),
                        streams: Arc::clone(&self.streams),
                        in_flight: self.in_flight.clone(),
                        #[cfg(test)]
                        mutation_hooks: self.mutation_hooks.clone(),
                    };
                    tasks.spawn(handle_connection(stream, context, admission));
                }
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(result) = joined { let _connection_result = result?; }
                }
            }
        };
        // Agent tool calls run over these connections, so they learn of the shutdown before
        // the connections close.
        if let Some(agents) = self.handle.agents() {
            agents.begin_shutdown();
        }
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

/// The pool a connection's slot came from, which decides what it may ask.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Anything.
    Business,
    /// Lifecycle calls and `service_accept` only.
    Accept,
    /// Lifecycle calls only.
    Lifecycle,
}

struct Admission {
    permit: OwnedSemaphorePermit,
    class: Class,
}

/// Admits a connection from the first pool with a free slot, in the order given.
///
/// A connection is admitted before its request is read, so a reserve slot is
/// taken on trust and its request is refused if it is not one the reserve serves.
/// `Ok(None)` means every slot of every pool is taken, which is the only case
/// where a connection is dropped unanswered.
fn admit(pools: [(&Arc<Semaphore>, Class); 3]) -> Result<Option<Admission>, IpcError> {
    for (pool, class) in pools {
        match Arc::clone(pool).try_acquire_owned() {
            Ok(permit) => return Ok(Some(Admission { permit, class })),
            Err(TryAcquireError::NoPermits) => {}
            Err(TryAcquireError::Closed) => return Err(IpcError::InvalidFrame),
        }
    }
    Ok(None)
}

struct ConnectionContext {
    paths: IpcPaths,
    handle: RuntimeHandle,
    control: CurrentUserRuntime,
    replay: Arc<Mutex<()>>,
    shutdown_sender: mpsc::Sender<()>,
    web: Option<crate::web::WebLifecycle>,
    identity: Arc<DaemonIdentity>,
    streams: Arc<Semaphore>,
    in_flight: crate::stream_plane::InFlight,
    #[cfg(test)]
    mutation_hooks: mutation_test_hooks::MutationTestHooks,
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
