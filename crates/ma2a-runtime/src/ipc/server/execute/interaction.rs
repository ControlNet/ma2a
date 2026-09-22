use ma2a_core::{EchoError, ProtocolError};

use crate::api::{Command, CommandResult, ControlSyncView, EchoReplyView};

use super::ConnectionContext;

pub(super) async fn control_sync_status(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ProtocolError> {
    let peer = command
        .control_sync_peer()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let synchronized = context
        .handle
        .control_sync_status(peer)
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    Ok(CommandResult::control_sync_status(
        ControlSyncView::new(if synchronized { vec![peer] } else { Vec::new() })
            .map_err(|_| ProtocolError::INTERNAL)?,
    ))
}

pub(super) async fn control_sync_trigger(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ProtocolError> {
    let peer = command
        .control_sync_peer()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    context
        .handle
        .sync_control_with(peer)
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    let synchronized = context
        .handle
        .control_sync_status(peer)
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    Ok(CommandResult::control_sync_triggered(
        ControlSyncView::new(if synchronized { vec![peer] } else { Vec::new() })
            .map_err(|_| ProtocolError::INTERNAL)?,
    ))
}

pub(super) async fn echo(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ProtocolError> {
    let (request_id, target, payload) = command.echo_call().ok_or(ProtocolError::INVALID_INPUT)?;
    let response = context
        .handle
        .echo(request_id, target, payload.as_bytes())
        .await
        .map_err(echo_protocol_error)?;
    let echoed =
        std::str::from_utf8(response.payload()).map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(CommandResult::echo(
        EchoReplyView::new(
            response.responder_endpoint_id(),
            echoed,
            response.duration_ms(),
        )
        .map_err(|_| ProtocolError::INTERNAL)?,
    ))
}

const fn echo_protocol_error(error: EchoError) -> ProtocolError {
    match error {
        EchoError::VersionMismatch => ProtocolError::VERSION_MISMATCH,
        EchoError::InvalidInput => ProtocolError::INVALID_INPUT,
        EchoError::Unauthorized => ProtocolError::UNAUTHORIZED,
        EchoError::ConcurrencyExceeded => ProtocolError::CONFLICT,
        EchoError::TimedOut | EchoError::Cancelled | EchoError::Unavailable => {
            ProtocolError::UNAVAILABLE
        }
        EchoError::Internal => ProtocolError::INTERNAL,
    }
}
