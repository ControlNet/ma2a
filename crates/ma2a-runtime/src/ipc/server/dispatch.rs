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

use crate::api::{self, HostedCommand};

use super::{
    Class, ConnectionContext, IpcError,
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
    admission: super::Admission,
) -> Result<(), IpcError> {
    let class = admission.class;
    // Windows names a pipe client only once something was read from the pipe, so the request is
    // read, within its bound, before the caller's account is checked; nothing is decoded or
    // answered until it is.
    let frame = read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
    platform::authorize(&stream, &context.paths)?;
    if let Ok(command) = api::decode_command(&frame.payload)
        && upgrades(class, &command)
    {
        let upgrade = ((frame.correlation, &command), admission.permit);
        return if command.is_hosted_upgrade() {
            super::hosted::upgrade(stream, &context, upgrade).await
        } else {
            super::stream::open(stream, &context, upgrade).await
        };
    }
    let _permit = admission.permit;
    let reserved = class != Class::Business;
    let (encoded, requests_shutdown) = answer(&frame.payload, &context, reserved).await?;
    if encoded.len() > api::MAX_LOCAL_RESPONSE_BYTES {
        let value: Value = serde_json::from_slice(&encoded).map_err(|_| IpcError::InvalidFrame)?;
        if !matches!(
            value.pointer("/result/type").and_then(Value::as_str),
            Some("snapshot" | "spaces")
        ) {
            return Err(IpcError::InvalidFrame);
        }
        let revision = value
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or(IpcError::InvalidFrame)?;
        let boot = value
            .get("runtime_boot_id")
            .and_then(Value::as_str)
            .ok_or(IpcError::InvalidFrame)?
            .to_owned();
        for fragment in api::fragments::SnapshotFragments::new(encoded, revision, boot) {
            write_frame(
                &mut stream,
                FrameRef {
                    correlation: frame.correlation,
                    payload: &fragment?,
                    maximum: api::MAX_LOCAL_RESPONSE_BYTES,
                },
            )
            .await?;
        }
    } else {
        write_frame(
            &mut stream,
            FrameRef {
                correlation: frame.correlation,
                payload: &encoded,
                maximum: api::MAX_LOCAL_RESPONSE_BYTES,
            },
        )
        .await?;
    }
    if requests_shutdown {
        // The receiver holds one slot and one request is enough, so a full
        // channel means shutdown is already under way. Never wait here: this is
        // the path a caller uses when the daemon has stopped answering.
        let _requested = context.shutdown_sender.try_send(());
    }
    Ok(())
}

/// Whether `command` upgrades the connection, which only the upgrades its slot serves do.
const fn upgrades(class: Class, command: &api::Command) -> bool {
    match class {
        Class::Business => command.stream_open().is_some() || command.is_hosted_upgrade(),
        Class::Accept => matches!(command.hosted_command(), Some(HostedCommand::Accept(..))),
        Class::Lifecycle => false,
    }
}

async fn answer(
    payload: &[u8],
    context: &ConnectionContext,
    reserved: bool,
) -> Result<(Vec<u8>, bool), IpcError> {
    if let Some(request) = LifecycleRequest::parse(payload) {
        return Ok(match request {
            LifecycleRequest::Ping => (context.identity.reply("pong"), false),
            LifecycleRequest::Stop => (context.identity.reply("stopping"), true),
        });
    }
    if reserved {
        // This connection came from a reserve, which exists so lifecycle calls and
        // hosted-call accepts survive saturation. Answering business traffic from it
        // would defeat that.
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
    // Only durable mutations (commands carrying a RequestId) serialize on the replay lock;
    // read-only queries, handshake, and UI lifecycle never queue behind slow mutations.
    let mut replay = if command.request_id().is_some() {
        Some(context.replay.lock().await)
    } else {
        None
    };
    let fingerprint = match command.request_id() {
        Some(request_id) => Some((request_id, api::command_fingerprint(&command)?)),
        None => None,
    };
    if let Some(request) = fingerprint {
        if let Some(answer) = replayed_or_reserved(context, request).await? {
            return Ok((answer, false));
        }
        // Remote network mutations and blocking Process/Agent supervisor waits can hold for up
        // to 70 s. They keep their durable reservation, which already refuses the same request as
        // pending, but release the lock so other local mutations are still answered meanwhile.
        if matches!(
            command.operation(),
            "space_leave"
                | "space_redeem"
                | "echo_call"
                | "control_sync_trigger"
                | "process_terminate"
                | "agent_define"
                | "agent_undefine"
                | "agent_stop"
                | "agent_start"
        ) {
            drop(replay.take());
        }
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
            let revision = authoritative_revision(&result, context).await?;
            (result, revision)
        }
        None => match execute(&command, &status, context).await {
            Ok(result) => {
                let revision = result
                    .committed_revision()
                    .or_else(|| api::snapshot_revision(&result))
                    .unwrap_or_else(|| status.revision());
                (result, revision)
            }
            Err(error) => return Ok((api::encode_error(error)?, false)),
        },
    };
    let response = api::ApiResponse::new(command.request_id(), response_revision, result);
    let encoded = if matches!(command.operation(), "snapshot_fetch" | "space_list") {
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
    Ok(encoded)
}

/// Answers a request ID that was already admitted, or durably reserves a new one and returns
/// `None` so that it executes.
async fn replayed_or_reserved(
    context: &ConnectionContext,
    (request_id, fingerprint): (ma2a_core::RequestId, [u8; 32]),
) -> Result<Option<Vec<u8>>, IpcError> {
    let Some(entry) = context.handle.mutation_replay(request_id).await? else {
        context
            .handle
            .reserve_mutation_replay(request_id, fingerprint)
            .await?;
        return Ok(None);
    };
    let refusal = match entry {
        ma2a_store::MutationReplayState::Completed(entry) if entry.fingerprint() == fingerprint => {
            return Ok(Some(entry.response().to_vec()));
        }
        ma2a_store::MutationReplayState::Pending(found) if found == fingerprint => {
            ProtocolError::UNAVAILABLE
        }
        ma2a_store::MutationReplayState::Pending(_)
        | ma2a_store::MutationReplayState::Completed(_) => ProtocolError::CONFLICT,
        _ => ProtocolError::UNAVAILABLE,
    };
    Ok(Some(api::encode_error(api::ApiError::new(refusal))?))
}
