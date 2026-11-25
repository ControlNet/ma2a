//! Stream-plane upgrade of one IPC connection: the Runtime dials the target as itself and
//! relays one Services v1 stream for a same-user client.

use std::time::Duration;

use ma2a_core::{
    EndpointId, RequestId, ServiceKind,
    service::{FrameKind, OpenHeader, ServiceError},
};
use ma2a_services::{Frame, FrameReader, FrameWriter, OperationEnd, OperationReport};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt as _},
    sync::OwnedSemaphorePermit,
};

use crate::{
    api::{self, ApiError, ApiResponse, Command, CommandResult},
    ipc::framing::{FrameRef, write_frame},
    stream_host::operations::{Direction, OperationKey, operation_name},
    stream_plane::{
        fingerprint,
        relay::{ClientWrite, Relayed, Terminal, bounded, relay},
    },
};

use super::{ConnectionContext, IpcError, platform::PlatformStream};

mod replay;

/// Longest wait for the client's `open` after the upgrade, and for the target to take it.
const OPEN_DEADLINE: Duration = Duration::from_secs(15);

/// Upgrades the connection, or answers a local API error and closes it.
///
/// The business permit is given up for a stream permit before anything streams, so a
/// long-running operation never occupies a business connection slot.
pub(super) async fn open(
    mut stream: PlatformStream,
    context: &ConnectionContext,
    ((correlation, command), business): ((u64, &Command), OwnedSemaphorePermit),
) -> Result<(), IpcError> {
    let (request_id, target, kind) = command.stream_open().ok_or(IpcError::InvalidFrame)?;
    let status = context.handle.status().await?;
    drop(business);
    let Some(stream_permit) = upgrade(&mut stream, context, (correlation, target, kind)).await?
    else {
        return Ok(());
    };
    let (read, write) = tokio::io::split(stream);
    let mut client = (FrameReader::new(read), FrameWriter::new(write));
    let Ok(Ok(frame)) = tokio::time::timeout(OPEN_DEADLINE, client.0.expect()).await else {
        drop(stream_permit);
        return Ok(());
    };
    let header = match validated(&frame, (request_id, target, kind)) {
        Ok(header) => header,
        Err(error) => {
            send_terminal(&mut client.1, &reject_frame(error)).await;
            drop(stream_permit);
            return Ok(());
        }
    };
    let key = OperationKey {
        request_id,
        peer: target,
        kind,
        operation: operation_name(header.body()),
        direction: Direction::Outbound,
    };
    let reservation = if header.body().is_mutation() {
        let decided = (request_id, fingerprint(kind, target, frame.payload()));
        match replay::admit(context, decided, status.revision()).await {
            replay::Admission::Proceed(reservation) => Some(reservation),
            replay::Admission::Answer(terminal) => {
                send_terminal(&mut client.1, &terminal).await;
                drop(stream_permit);
                return Ok(());
            }
        }
    } else {
        None
    };
    let stall = crate::stream_plane::stall_for(header.body());
    let operation = ((key, frame.payload(), stall), reservation);
    run(context, operation, (&mut client.0, &mut client.1)).await;
    drop(stream_permit);
    Ok(())
}

/// Answers the upgrade request; `None` means it was refused and the connection closes.
async fn upgrade(
    stream: &mut PlatformStream,
    context: &ConnectionContext,
    (correlation, target, kind): (u64, EndpointId, ServiceKind),
) -> Result<Option<OwnedSemaphorePermit>, IpcError> {
    let status = context.handle.status().await?;
    let permit = if target == status.endpoint_id() || kind == ServiceKind::Echo {
        Err(ApiError::invalid_input())
    } else {
        std::sync::Arc::clone(&context.streams)
            .try_acquire_owned()
            .map_err(|_| ApiError::new(ma2a_core::ProtocolError::UNAVAILABLE))
    };
    let encoded = match &permit {
        Ok(_) => api::encode_response(&ApiResponse::new(
            None,
            status.revision(),
            CommandResult::stream_opened(),
        ))?,
        Err(error) => api::encode_error(*error)?,
    };
    write_frame(
        stream,
        FrameRef {
            correlation,
            payload: &encoded,
            maximum: api::MAX_LOCAL_RESPONSE_BYTES,
        },
    )
    .await?;
    Ok(permit.ok())
}

fn validated(
    frame: &Frame,
    (request_id, target, kind): (RequestId, EndpointId, ServiceKind),
) -> Result<OpenHeader, ServiceError> {
    if frame.kind() != FrameKind::Open {
        return Err(ServiceError::InvalidInput);
    }
    let header = OpenHeader::decode(frame.payload()).map_err(ServiceError::from_protocol)?;
    if header.request_id() == request_id
        && header.target() == target
        && header.body().kind() == kind
    {
        Ok(header)
    } else {
        Err(ServiceError::InvalidInput)
    }
}

/// Deterministic fault point: the target stream is lost right after `open` was written.
#[cfg(test)]
fn injected_loss(context: &ConnectionContext) -> bool {
    context.mutation_hooks.take_stream_loss()
}

#[cfg(not(test))]
const fn injected_loss(_context: &ConnectionContext) -> bool {
    false
}

#[cfg(test)]
async fn injected_stall(context: &ConnectionContext) {
    if context.mutation_hooks.take_dial_stall() {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(test))]
#[expect(
    clippy::unused_async,
    reason = "the test build stalls here; production code awaits nothing"
)]
async fn injected_stall(_context: &ConnectionContext) {}

fn reject_frame(error: ServiceError) -> Vec<u8> {
    crate::stream_plane::relay::encoded(FrameKind::Reject, &[error.code()]).unwrap_or_default()
}

/// Writes one exact terminal frame and half-closes; the client may already be gone.
async fn send_terminal<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>, terminal: &[u8]) {
    let inner = writer.get_mut();
    let _written = bounded(inner.write_all(terminal)).await;
    let _closed = bounded(inner.shutdown()).await;
}

async fn run<R, W>(
    context: &ConnectionContext,
    ((key, open, stall), reservation): (
        (OperationKey, &[u8], Duration),
        Option<replay::Reservation>,
    ),
    (client_reader, client_writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let guard = context.handle.operations().start(key);
    // Dialing and writing `open` share one deadline, so a peer that grants no stream holds
    // neither the client's slot nor its reservation; nothing has reached it before `open`.
    let deadline = tokio::time::Instant::now() + OPEN_DEADLINE;
    let dial = async {
        injected_stall(context).await;
        match context.handle.service_client().await {
            Ok(client) => client.open(key.peer, key.kind).await,
            Err(_) => Err(ServiceError::Unavailable),
        }
    };
    let dialed = tokio::time::timeout_at(deadline, dial)
        .await
        .unwrap_or(Err(ServiceError::TimedOut));
    let mut remote = match dialed {
        Ok(remote) => remote,
        Err(error) => {
            if let Some(reservation) = reservation {
                reservation.release().await;
            }
            send_terminal(client_writer, &reject_frame(error)).await;
            guard.finish(OperationReport::new(OperationEnd::Rejected(error)));
            return;
        }
    };
    let (send, receive) = remote.halves();
    let mut target = (FrameReader::new(receive), FrameWriter::new(send));
    // A target that gives no room for the request cannot hold the client's slot.
    let open_write = async {
        target.1.frame(FrameKind::Open, open).await?;
        target.1.flush().await
    };
    let opened = matches!(
        tokio::time::timeout_at(deadline, open_write).await,
        Ok(Ok(()))
    ) && !injected_loss(context);
    let relayed = if opened {
        relay(
            (client_reader, client_writer),
            (&mut target.0, &mut target.1),
            (|| (), &ClientWrite::default(), stall),
        )
        .await
    } else {
        Relayed {
            terminal: Terminal::Lost,
            to_target: 0,
            from_target: 0,
            client_present: true,
        }
    };
    let end = match relayed.terminal {
        Terminal::Rejected(error) => {
            if let Some(reservation) = reservation {
                reservation.release().await;
            }
            OperationEnd::Rejected(error)
        }
        Terminal::Outcome { frame, failure } => {
            if let Some(reservation) = reservation {
                reservation.complete(frame).await;
            }
            failure.map_or(OperationEnd::Completed, OperationEnd::Failed)
        }
        Terminal::Lost => {
            // Nothing proves the target did not act, so the target is reset and the request
            // is settled as unknown rather than released for a retry that could act twice.
            let _reset = target.1.get_mut().reset(0_u8.into());
            let terminal = replay::outcome_unknown();
            if relayed.client_present {
                send_terminal(client_writer, &terminal).await;
            }
            if let Some(reservation) = reservation {
                reservation.complete(terminal).await;
            }
            OperationEnd::Failed(ServiceError::OutcomeUnknown)
        }
    };
    let _closed = bounded(ma2a_services::reply::close(client_writer)).await;
    guard.finish(OperationReport::new(end).with_bytes(relayed.to_target, relayed.from_target));
}
