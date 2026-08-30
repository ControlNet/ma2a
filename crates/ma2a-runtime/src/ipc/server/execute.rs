use ma2a_core::{EchoError, ProtocolError};

use crate::{
    RuntimeStatus,
    api::{
        CapabilityFlags, Command, CommandResult, ControlSyncView, EchoReplyView, EndpointView,
        HandshakeAuth, HandshakeState, HandshakeView, InteractionCapabilities,
        ManagementCapabilities, RelayCapabilities, RuntimeStatusView,
    },
};

use super::{ConnectionContext, IpcError};

pub(super) async fn authoritative_revision(
    command: &Command,
    current_revision: u64,
    context: &ConnectionContext,
) -> Result<u64, IpcError> {
    match command.operation() {
        "ui_password_set" | "ui_password_reset" | "session_revoke_all" => {
            let persisted = context
                .control
                .state_revision()
                .await
                .map_err(|_| IpcError::InvalidFrame)?;
            context
                .handle
                .adopt_revision(persisted)
                .await
                .map_err(IpcError::from)
        }
        "control_sync_trigger" => context
            .handle
            .status()
            .await
            .map(|status| status.revision())
            .map_err(IpcError::from),
        _ => Ok(current_revision),
    }
}

const fn capabilities() -> CapabilityFlags {
    CapabilityFlags::new(
        ManagementCapabilities::new(false, false),
        RelayCapabilities::new(false, false),
        InteractionCapabilities::new(false, false),
    )
}

pub(super) async fn execute(
    command: &Command,
    status: &RuntimeStatus,
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    Ok(match command.operation() {
        "handshake" => CommandResult::handshake(
            HandshakeView::new(
                env!("CARGO_PKG_VERSION"),
                status.endpoint_id(),
                HandshakeState::new(
                    status.revision(),
                    HandshakeAuth::new(
                        true,
                        context
                            .control
                            .password_is_set()
                            .await
                            .map_err(|_| ProtocolError::INTERNAL)?,
                    ),
                    capabilities(),
                ),
            )
            .map_err(|_| ProtocolError::INTERNAL)?,
        ),
        "status" => CommandResult::status(RuntimeStatusView::new(status.revision(), true, false)),
        "endpoint_info" => CommandResult::endpoint_info(
            EndpointView::new(
                status.endpoint_id(),
                env!("CARGO_PKG_VERSION"),
                status.is_ready(),
            )
            .map_err(|_| ProtocolError::INTERNAL)?,
        ),
        "control_sync_status" => {
            let peer = command
                .control_sync_peer()
                .ok_or(ProtocolError::INVALID_INPUT)?;
            let synchronized = context
                .handle
                .control_sync_status(peer)
                .await
                .map_err(|_| ProtocolError::UNAVAILABLE)?;
            CommandResult::control_sync_status(
                ControlSyncView::new(vec![peer], synchronized)
                    .map_err(|_| ProtocolError::INTERNAL)?,
            )
        }
        "control_sync_trigger" => {
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
            CommandResult::control_sync_triggered(
                ControlSyncView::new(vec![peer], synchronized)
                    .map_err(|_| ProtocolError::INTERNAL)?,
            )
        }
        "echo_call" => {
            let (request_id, target, payload) =
                command.echo_call().ok_or(ProtocolError::INVALID_INPUT)?;
            let response = context
                .handle
                .echo(request_id, target, payload.as_bytes())
                .await
                .map_err(echo_protocol_error)?;
            let echoed = std::str::from_utf8(response.payload())
                .map_err(|_| ProtocolError::INVALID_INPUT)?;
            CommandResult::echo(
                EchoReplyView::new(response.responder_endpoint_id(), echoed)
                    .map_err(|_| ProtocolError::INTERNAL)?,
            )
        }
        "graceful_shutdown" => CommandResult::shutting_down(),
        "ui_password_set" | "ui_password_reset" | "session_revoke_all" => context
            .control
            .send(command.clone())
            .await
            .map_err(|_| ProtocolError::INTERNAL)?,
        _ => return Err(ProtocolError::UNAVAILABLE),
    })
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
