//! Deciding what one accepted connection is asking for, and answering it.
//!
//! Two planes meet here and are kept apart on purpose. A lifecycle call is
//! answered from the identity record the daemon fixed at startup and returns
//! before touching the Runtime at all: no replay lock, no actor mailbox, no
//! store, no network. Everything else is a business command and keeps the
//! existing durable-replay and actor path. A caller asking whether the daemon is
//! alive must never queue behind the work that made it doubt.

use ma2a_core::ProtocolError;
use serde_json::Value;

use crate::api;

use super::{
    ConnectionContext, IpcError,
    execute::{authoritative_revision, execute},
    platform,
};
use crate::ipc::{
    framing::{FrameRef, read_frame, write_frame},
    lifecycle::LifecycleRequest,
};

pub(super) async fn handle_connection(
    mut stream: platform::PlatformStream,
    context: ConnectionContext,
    lifecycle_only: bool,
) -> Result<(), IpcError> {
    platform::authorize(&stream, &context.paths)?;
    let frame = read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
    let (encoded, requests_shutdown) = answer(&frame.payload, &context, lifecycle_only).await?;
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
        // The receiver holds one slot and one request is enough, so a full
        // channel means shutdown is already under way. Never wait here: this is
        // the path a caller uses when the daemon has stopped answering.
        let _requested = context.shutdown_sender.try_send(());
    }
    Ok(())
}

async fn answer(
    payload: &[u8],
    context: &ConnectionContext,
    lifecycle_only: bool,
) -> Result<(Vec<u8>, bool), IpcError> {
    if let Some(request) = LifecycleRequest::parse(payload) {
        return Ok(match request {
            LifecycleRequest::Ping => (context.identity.reply("pong"), false),
            LifecycleRequest::Stop => (context.identity.reply("stopping"), true),
        });
    }
    if lifecycle_only {
        // This connection came from the reserve, which exists so lifecycle calls
        // survive saturation. Answering business traffic from it would defeat that.
        return Ok((
            api::encode_error(api::ApiError::new(ProtocolError::UNAVAILABLE))?,
            false,
        ));
    }
    match api::decode_command(payload) {
        Ok(_) => dispatch(payload, context).await,
        Err(error) => Ok((api::encode_error(error)?, false)),
    }
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
                    return Ok((response, false));
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
                    let encoded = super::mutation_outcome::record_failure(
                        context,
                        (
                            ma2a_store::MutationReplayRequest::new(
                                request_id,
                                fingerprint
                                    .map(|(_, value)| value)
                                    .ok_or(IpcError::InvalidFrame)?,
                            ),
                            status.revision(),
                        ),
                        error,
                    )
                    .await?;
                    return Ok((encoded, false));
                }
            };
            let revision =
                authoritative_revision(&command, (&result, status.revision()), context).await?;
            (result, revision)
        }
        None => match execute(&command, &status, context).await {
            Ok(result) => {
                let revision = api::snapshot_revision(&result).unwrap_or_else(|| status.revision());
                (result, revision)
            }
            Err(error) => return Ok((api::encode_error(error)?, false)),
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
