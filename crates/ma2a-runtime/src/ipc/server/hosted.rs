//! Hosted-service upgrades of one IPC connection: a service's link, and each call it accepts.
//!
//! A link carries local API frames with one small JSON message each. The service sends
//! `{"t":"ready"}` or `{"t":"unavailable"}`; the Runtime sends `{"t":"call","call":"<hex>"}`
//! for every call waiting to be accepted, and `{"t":"stop"}` once before it ends the link.

use ma2a_core::{
    ProtocolError,
    service::{ServiceError, ServiceName},
};
use serde_json::{Value, json};
use tokio::sync::OwnedSemaphorePermit;

use crate::{
    api::{self, ApiError, ApiResponse, Command, CommandResult, HostedCommand},
    hosting::{Accepted, HostedServices, LinkHandle, Notice},
    ipc::{
        MAX_LINK_MESSAGE_BYTES,
        framing::{FrameRef, encode_frame, read_reply, write_frame},
    },
};

use super::{ConnectionContext, IpcError, platform::PlatformStream};

/// Ends the link however its connection task ends, even when it is aborted.
struct Detach {
    hosted: HostedServices,
    name: ServiceName,
    id: u64,
}

impl Drop for Detach {
    fn drop(&mut self) {
        self.hosted.detach(&self.name, self.id);
    }
}

/// Upgrades the connection, or answers a local API error and closes it.
pub(super) async fn upgrade(
    mut stream: PlatformStream,
    context: &ConnectionContext,
    ((correlation, command), slot): ((u64, &Command), OwnedSemaphorePermit),
) -> Result<(), IpcError> {
    let revision = context.handle.status().await?.revision();
    let Some(hosted) = context.handle.hosted() else {
        let error = ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "this Runtime does not host services",
        );
        return answer(&mut stream, correlation, &api::encode_error(error)?).await;
    };
    match command.hosted_command() {
        Some(HostedCommand::Attach(name)) => {
            let (link, manifest) = match hosted.attach(name).await {
                Ok(attached) => attached,
                Err(error) => {
                    let refused = api::encode_error(refusal(error))?;
                    return answer(&mut stream, correlation, &refused).await;
                }
            };
            let guard = Detach {
                hosted: hosted.clone(),
                name: name.clone(),
                id: link.id,
            };
            drop(slot);
            let result = CommandResult::service_attached(name, &manifest);
            let attached = api::encode_response(&ApiResponse::new(None, revision, result))?;
            answer(&mut stream, correlation, &attached).await?;
            serve_link(stream, (hosted, name), (correlation, link)).await;
            drop(guard);
            Ok(())
        }
        Some(HostedCommand::Accept(name, call)) => {
            let (details, deliver) = match hosted.accept(name, *call) {
                Ok(accepted) => accepted,
                Err(error) => {
                    let refused = api::encode_error(refusal(error))?;
                    return answer(&mut stream, correlation, &refused).await;
                }
            };
            let result = CommandResult::service_call(
                (name, *call),
                (details.caller, details.request_id),
                (details.version, &details.operation, &details.body),
            );
            let encoded = api::encode_response(&ApiResponse::new(None, revision, result))?;
            // The call writes this answer itself, so it is never both delivered and withdrawn.
            let answer = encode_frame(&FrameRef {
                correlation,
                payload: &encoded,
                maximum: api::MAX_LOCAL_RESPONSE_BYTES,
            })?;
            drop(slot);
            let _delivered = deliver.send(Accepted {
                stream: Box::new(stream),
                answer,
            });
            Ok(())
        }
        _ => Err(IpcError::InvalidFrame),
    }
}

async fn answer(
    stream: &mut PlatformStream,
    correlation: u64,
    payload: &[u8],
) -> Result<(), IpcError> {
    write_frame(
        stream,
        FrameRef {
            correlation,
            payload,
            maximum: api::MAX_LOCAL_RESPONSE_BYTES,
        },
    )
    .await
}

/// Carries one link until either side ends it: the service's reports in, notices out.
async fn serve_link(
    stream: PlatformStream,
    (hosted, name): (&HostedServices, &ServiceName),
    (correlation, mut link): (u64, LinkHandle),
) {
    let id = link.id;
    let (mut read, mut write) = tokio::io::split(stream);
    let reading = async {
        while let Ok(frame) = read_reply(&mut read, MAX_LINK_MESSAGE_BYTES).await {
            match report(&frame.payload) {
                Some(ready) => hosted.report((name, id), ready),
                None => return,
            }
        }
    };
    let writing = async {
        while let Some(notice) = link.next().await {
            let message = match notice {
                Notice::Call(call) => json!({"t": "call", "call": api::encode_hex(&call)}),
                Notice::Stop => json!({"t": "stop"}),
            };
            let Ok(payload) = serde_json::to_vec(&message) else {
                return;
            };
            let frame = FrameRef {
                correlation,
                payload: &payload,
                maximum: MAX_LINK_MESSAGE_BYTES,
            };
            if write_frame(&mut write, frame).await.is_err() || notice == Notice::Stop {
                return;
            }
        }
    };
    tokio::select! {
        () = reading => {}
        () = writing => {}
    }
}

/// Reads one service report: whether it accepts calls now; `None` for anything else.
fn report(payload: &[u8]) -> Option<bool> {
    let message: Value = serde_json::from_slice(payload).ok()?;
    let object = message.as_object().filter(|object| object.len() == 1)?;
    match object.get("t").and_then(Value::as_str)? {
        "ready" => Some(true),
        "unavailable" => Some(false),
        _ => None,
    }
}

/// Maps a refused upgrade onto the local API error set.
const fn refusal(error: ServiceError) -> ApiError {
    match error {
        ServiceError::UnknownService => ApiError::with_remediation(
            ProtocolError::NOT_FOUND,
            "no such hosted service is registered",
        ),
        ServiceError::NotFound => ApiError::with_remediation(
            ProtocolError::NOT_FOUND,
            "no such call waits for this service",
        ),
        ServiceError::Unavailable => ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "the service is stopped; start it first",
        ),
        ServiceError::Conflict => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "another process is attached to this service",
        ),
        _ => ApiError::new(ProtocolError::UNAVAILABLE),
    }
}
