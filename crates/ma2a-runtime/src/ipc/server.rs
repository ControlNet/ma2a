use std::{collections::BTreeMap, sync::Arc};

use ma2a_core::{ProtocolError, RequestId};
use tokio::{
    sync::{Mutex, Semaphore, TryAcquireError, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeHandle,
    api::{self, CommandResult},
    current_user::CurrentUserRuntime,
};

use super::{
    CONNECTION_LIMIT, IpcError, IpcPaths,
    framing::{FrameRef, read_frame, write_frame},
    platform,
};

mod execute;
use execute::{authoritative_revision, execute};

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
    control: CurrentUserRuntime,
    replay: Arc<Mutex<BTreeMap<RequestId, ReplayEntry>>>,
}

impl LocalApiServer {
    /// Binds the platform endpoint after validating its private location.
    ///
    /// # Errors
    /// Returns a platform transport or authorization error when binding fails.
    pub fn bind(
        paths: IpcPaths,
        handle: RuntimeHandle,
        control: CurrentUserRuntime,
    ) -> Result<Self, IpcError> {
        let listener = platform::bind(&paths)?;
        Ok(Self {
            listener,
            paths,
            handle,
            control,
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
                        control: self.control.clone(),
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
    control: CurrentUserRuntime,
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
    let (encoded, requests_shutdown) = match preflight {
        Ok(_) => dispatch(&frame.payload, &context).await?,
        Err(error) => (api::encode_error(error)?, false),
    };
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

async fn dispatch(input: &[u8], context: &ConnectionContext) -> Result<(Vec<u8>, bool), IpcError> {
    let command = api::decode_command(input)?;
    let status = context.handle.status().await?;
    let mut replay = context.replay.lock().await;
    let fingerprint = match command.request_id() {
        Some(request_id) => Some((request_id, api::command_fingerprint(&command)?)),
        None => None,
    };
    let (result, response_revision) = match fingerprint {
        Some((request_id, fingerprint)) => match replay.get(&request_id) {
            Some(entry) if entry.fingerprint == fingerprint => {
                (entry.result.clone(), entry.revision)
            }
            Some(_) => {
                return Ok((
                    api::encode_error(api::ApiError::new(ProtocolError::CONFLICT))?,
                    false,
                ));
            }
            None => {
                let result = match execute(&command, &status, context).await {
                    Ok(result) => result,
                    Err(error) => {
                        return Ok((api::encode_error(api::ApiError::new(error))?, false));
                    }
                };
                let revision = authoritative_revision(&command, status.revision(), context).await?;
                replay.insert(
                    request_id,
                    ReplayEntry {
                        fingerprint,
                        result: result.clone(),
                        revision,
                    },
                );
                (result, revision)
            }
        },
        None => match execute(&command, &status, context).await {
            Ok(result) => {
                let revision = api::snapshot_revision(&result).unwrap_or_else(|| status.revision());
                (result, revision)
            }
            Err(error) => return Ok((api::encode_error(api::ApiError::new(error))?, false)),
        },
    };
    drop(replay);
    let response = api::ApiResponse::new(command.request_id(), response_revision, result);
    Ok((
        api::encode_response(&response)?,
        response.result_type() == "shutting_down",
    ))
}

#[derive(Clone, Debug)]
struct ReplayEntry {
    fingerprint: [u8; 32],
    result: CommandResult,
    revision: u64,
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
