use std::sync::Arc;

use ma2a_core::ProtocolError;
use serde_json::Value;
use tokio::{
    sync::{Mutex, Semaphore, TryAcquireError, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeHandle,
    api::{self},
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
    replay: Arc<Mutex<()>>,
    web: Option<crate::web::WebLifecycle>,
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
            replay: Arc::new(Mutex::new(())),
            web: None,
        })
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
                        web: self.web.clone(),
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
    replay: Arc<Mutex<()>>,
    shutdown_sender: mpsc::Sender<()>,
    web: Option<crate::web::WebLifecycle>,
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
    // Volatile lifecycle instructions execute on every call and never enter durable replay.
    // They must not hold the mutation lock while closing HTTP requests using this IPC.
    let _replay = if command.is_ui_lifecycle() {
        None
    } else {
        Some(context.replay.lock().await)
    };
    let fingerprint = match command.request_id() {
        Some(request_id) => Some((request_id, api::command_fingerprint(&command)?)),
        None => None,
    };
    if let Some((request_id, fingerprint)) = fingerprint {
        if let Some(entry) = context.handle.mutation_replay(request_id).await? {
            match entry {
                ma2a_store::MutationReplayState::Pending(found) if found == fingerprint => {
                    return Ok((
                        api::encode_error(api::ApiError::new(ProtocolError::UNAVAILABLE))?,
                        false,
                    ));
                }
                ma2a_store::MutationReplayState::Completed(entry)
                    if entry.fingerprint() == fingerprint =>
                {
                    let response = entry.response().to_vec();
                    return Ok((response.clone(), response_requests_shutdown(&response)?));
                }
                ma2a_store::MutationReplayState::Pending(_)
                | ma2a_store::MutationReplayState::Completed(_) => {
                    return Ok((
                        api::encode_error(api::ApiError::new(ProtocolError::CONFLICT))?,
                        false,
                    ));
                }
                _ => {
                    return Ok((
                        api::encode_error(api::ApiError::new(ProtocolError::UNAVAILABLE))?,
                        false,
                    ));
                }
            }
        }
        context
            .handle
            .reserve_mutation_replay(request_id, fingerprint)
            .await?;
    }
    let status = context.handle.status().await?;
    let (result, response_revision) = match fingerprint {
        Some((request_id, _)) => {
            let result = match execute(&command, &status, context).await {
                Ok(result) => result,
                Err(error) => {
                    context.handle.abort_mutation_replay(request_id).await?;
                    return Ok((api::encode_error(api::ApiError::new(error))?, false));
                }
            };
            let revision = authoritative_revision(&command, status.revision(), context).await?;
            (result, revision)
        }
        None => match execute(&command, &status, context).await {
            Ok(result) => {
                let revision = api::snapshot_revision(&result).unwrap_or_else(|| status.revision());
                (result, revision)
            }
            Err(error) => return Ok((api::encode_error(api::ApiError::new(error))?, false)),
        },
    };
    let response = api::ApiResponse::new(command.request_id(), response_revision, result);
    let encoded = if command.operation() == "snapshot_fetch" {
        encode_stamped_snapshot_response(&response, status.boot_id())?
    } else {
        api::encode_response(&response)?
    };
    if let Some((request_id, fingerprint)) = fingerprint {
        let record = ma2a_store::MutationReplayRecord::new(
            ma2a_store::MutationReplayRequest::new(request_id, fingerprint),
            response_revision,
            encoded.clone(),
        )
        .map_err(crate::RuntimeError::from)?;
        context.handle.record_mutation_replay(record).await?;
    }
    Ok((encoded, response.result_type() == "shutting_down"))
}

fn response_requests_shutdown(response: &[u8]) -> Result<bool, IpcError> {
    let value: Value = serde_json::from_slice(response).map_err(|_| IpcError::InvalidFrame)?;
    Ok(value.pointer("/result/type").and_then(Value::as_str) == Some("shutting_down"))
}

fn encode_stamped_snapshot_response(
    response: &api::ApiResponse,
    boot_id: [u8; 16],
) -> Result<Vec<u8>, IpcError> {
    let encoded = api::encode_response(response)?;
    let mut value: Value = serde_json::from_slice(&encoded).map_err(|_| IpcError::InvalidFrame)?;
    value.as_object_mut().ok_or(IpcError::InvalidFrame)?.insert(
        "runtime_boot_id".to_owned(),
        Value::String(api::encode_hex(&boot_id)),
    );
    let encoded = serde_json::to_vec(&value).map_err(|_| IpcError::InvalidFrame)?;
    if encoded.len() > api::MAX_LOCAL_RESPONSE_BYTES {
        Err(IpcError::InvalidFrame)
    } else {
        Ok(encoded)
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
