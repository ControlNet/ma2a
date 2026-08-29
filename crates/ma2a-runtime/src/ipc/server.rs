use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

use ma2a_core::{ProtocolError, RequestId};
use tokio::{
    sync::{Semaphore, TryAcquireError, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeHandle, RuntimeStatus,
    api::{
        self, CapabilityFlags, Command, CommandResult, EndpointView, HandshakeAuth, HandshakeState,
        HandshakeView, InteractionCapabilities, ManagementCapabilities, RelayCapabilities,
        ReplayDecision, RuntimeApiBoundary, RuntimeStatusView,
    },
};

use super::{
    CONNECTION_LIMIT, IpcError, IpcPaths,
    framing::{FrameRef, read_frame, write_frame},
    platform,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
/// Reason the local server stopped accepting connections.
pub enum ServerExit {
    /// The parent cancellation token was cancelled.
    Cancelled,
    /// A client completed the graceful shutdown command.
    ShutdownRequested,
}

#[derive(Debug)]
/// Current-user local server backed by one Runtime handle.
pub struct LocalApiServer {
    listener: platform::PlatformListener,
    paths: IpcPaths,
    handle: RuntimeHandle,
    replay: Arc<Mutex<BTreeMap<RequestId, ReplayEntry>>>,
}

impl LocalApiServer {
    /// Binds the platform endpoint after validating its private location.
    ///
    /// # Errors
    /// Returns a platform transport or authorization error when binding fails.
    pub fn bind(paths: IpcPaths, handle: RuntimeHandle) -> Result<Self, IpcError> {
        let listener = platform::bind(&paths)?;
        Ok(Self {
            listener,
            paths,
            handle,
            replay: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Serves bounded concurrent connections until cancellation or graceful shutdown.
    ///
    /// # Errors
    /// Returns transport, Runtime, framing, or connection task failures.
    pub async fn serve(self, cancellation: CancellationToken) -> Result<ServerExit, IpcError> {
        let permits = std::sync::Arc::new(Semaphore::new(CONNECTION_LIMIT));
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);
        let mut tasks = JoinSet::new();
        let exit = loop {
            tokio::select! {
                () = cancellation.cancelled() => break ServerExit::Cancelled,
                shutdown = shutdown_receiver.recv() => {
                    if shutdown.is_some() { break ServerExit::ShutdownRequested; }
                }
                accepted = platform::accept(&self.listener) => {
                    let stream = accepted?;
                    let permit = match std::sync::Arc::clone(&permits).try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(TryAcquireError::NoPermits) => continue,
                        Err(TryAcquireError::Closed) => return Err(IpcError::InvalidFrame),
                    };
                    let context = ConnectionContext {
                        paths: self.paths.clone(),
                        handle: self.handle.clone(),
                        replay: Arc::clone(&self.replay),
                        shutdown_sender: shutdown_sender.clone(),
                    };
                    tasks.spawn(async move {
                        let _permit = permit;
                        handle_connection(stream, context).await
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

struct ConnectionContext {
    paths: IpcPaths,
    handle: RuntimeHandle,
    replay: Arc<Mutex<BTreeMap<RequestId, ReplayEntry>>>,
    shutdown_sender: mpsc::Sender<()>,
}

async fn handle_connection(
    mut stream: platform::PlatformStream,
    context: ConnectionContext,
) -> Result<(), IpcError> {
    platform::authorize(&stream, &context.paths)?;
    let frame = read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
    let preflight = api::decode_command(&frame.payload);
    let encoded = match preflight {
        Ok(_) => dispatch(&frame.payload, context.handle, &context.replay).await?,
        Err(error) => api::encode_error(error)?,
    };
    let requests_shutdown =
        preflight.is_ok_and(|command| command.operation() == "graceful_shutdown");
    write_frame(
        &mut stream,
        FrameRef {
            correlation: frame.correlation,
            payload: &encoded,
            maximum: api::MAX_LOCAL_RESPONSE_BYTES,
        },
    )
    .await?;
    if requests_shutdown {
        let _send_result = context.shutdown_sender.send(()).await;
    }
    Ok(())
}

async fn dispatch(
    input: &[u8],
    handle: RuntimeHandle,
    replay: &Mutex<BTreeMap<RequestId, ReplayEntry>>,
) -> Result<Vec<u8>, IpcError> {
    let status = handle.status().await?;
    dispatch_locked(
        input,
        status,
        match replay.lock() {
            Ok(replay) => replay,
            Err(poisoned) => poisoned.into_inner(),
        },
    )
}

fn dispatch_locked(
    input: &[u8],
    status: RuntimeStatus,
    replay: MutexGuard<'_, BTreeMap<RequestId, ReplayEntry>>,
) -> Result<Vec<u8>, IpcError> {
    let mut boundary = RuntimeBoundary::new(status, replay);
    match api::dispatch_request(input, &mut boundary) {
        Ok(response) => Ok(api::encode_response(&response)?),
        Err(error) => Ok(api::encode_error(error)?),
    }
}

#[derive(Clone, Debug)]
struct ReplayEntry {
    fingerprint: [u8; 32],
    result: CommandResult,
}

struct RuntimeBoundary<'a> {
    status: RuntimeStatus,
    replay: MutexGuard<'a, BTreeMap<RequestId, ReplayEntry>>,
    fresh_fingerprint: Option<(RequestId, [u8; 32])>,
}

impl<'a> RuntimeBoundary<'a> {
    const fn new(
        status: RuntimeStatus,
        replay: MutexGuard<'a, BTreeMap<RequestId, ReplayEntry>>,
    ) -> Self {
        Self {
            status,
            replay,
            fresh_fingerprint: None,
        }
    }

    const fn capabilities() -> CapabilityFlags {
        CapabilityFlags::new(
            ManagementCapabilities::new(false, false),
            RelayCapabilities::new(false, false),
            InteractionCapabilities::new(false, false),
        )
    }
}

impl RuntimeApiBoundary for RuntimeBoundary<'_> {
    fn state_revision(&mut self) -> u64 {
        self.status.revision()
    }

    fn replay_decision(&mut self, request_id: RequestId, fingerprint: [u8; 32]) -> ReplayDecision {
        match self.replay.get(&request_id) {
            Some(entry) if entry.fingerprint == fingerprint => ReplayDecision::REPLAY,
            Some(_) => ReplayDecision::CONFLICT,
            None => {
                self.fresh_fingerprint = Some((request_id, fingerprint));
                ReplayDecision::FRESH
            }
        }
    }

    fn replay_result(&mut self, request_id: RequestId) -> Result<CommandResult, ProtocolError> {
        self.replay
            .get(&request_id)
            .map(|entry| entry.result.clone())
            .ok_or(ProtocolError::CONFLICT)
    }

    fn execute(&mut self, command: &Command) -> Result<CommandResult, ProtocolError> {
        let result = match command.operation() {
            "handshake" => CommandResult::handshake(
                HandshakeView::new(
                    env!("CARGO_PKG_VERSION"),
                    self.status.endpoint_id(),
                    HandshakeState::new(
                        self.status.revision(),
                        HandshakeAuth::new(true, false),
                        Self::capabilities(),
                    ),
                )
                .map_err(|_| ProtocolError::INTERNAL)?,
            ),
            "status" => {
                CommandResult::status(RuntimeStatusView::new(self.status.revision(), true, false))
            }
            "endpoint_info" => CommandResult::endpoint_info(
                EndpointView::new(
                    self.status.endpoint_id(),
                    env!("CARGO_PKG_VERSION"),
                    self.status.is_ready(),
                )
                .map_err(|_| ProtocolError::INTERNAL)?,
            ),
            "graceful_shutdown" => CommandResult::shutting_down(),
            _ => return Err(ProtocolError::UNAVAILABLE),
        };
        if let Some(request_id) = command.request_id() {
            let fingerprint = self
                .fresh_fingerprint
                .take()
                .filter(|(fresh_id, _fingerprint)| *fresh_id == request_id)
                .map(|(_fresh_id, fingerprint)| fingerprint)
                .ok_or(ProtocolError::CONFLICT)?;
            self.replay.insert(
                request_id,
                ReplayEntry {
                    fingerprint,
                    result: result.clone(),
                },
            );
        }
        Ok(result)
    }
}
