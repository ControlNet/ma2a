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

mod relay;
mod space;

pub(super) async fn authoritative_revision(
    command: &Command,
    current_revision: u64,
    context: &ConnectionContext,
) -> Result<u64, IpcError> {
    match command.operation() {
        "ui_init" | "ui_password_set" | "ui_password_reset" | "session_revoke_all" => {
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
        operation if uses_post_execution_actor_revision(operation) => context
            .handle
            .status()
            .await
            .map(|status| status.revision())
            .map_err(IpcError::from),
        _ => Ok(current_revision),
    }
}

fn uses_post_execution_actor_revision(operation: &str) -> bool {
    matches!(
        operation,
        "space_create"
            | "space_invite"
            | "space_redeem"
            | "space_revoke"
            | "private_relay_configure"
            | "private_relay_disable"
            | "public_relay_configure"
            | "public_relay_disable"
            | "control_sync_trigger"
            | "echo_call"
    )
}

const fn capabilities() -> CapabilityFlags {
    CapabilityFlags::new(
        ManagementCapabilities::new(false, false),
        RelayCapabilities::new(false, false),
        InteractionCapabilities::new(false, false),
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "closed local API dispatch remains explicit"
)]
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
        "space_create" => {
            space::create(
                context,
                command
                    .space_create_name()
                    .ok_or(ProtocolError::INVALID_INPUT)?,
            )
            .await?
        }
        "space_list" => space::list(context).await?,
        "space_show" => {
            space::show(
                context,
                command
                    .space_show_id()
                    .ok_or(ProtocolError::INVALID_INPUT)?,
            )
            .await?
        }
        "space_invite" => space::invite(context, command).await?,
        "space_redeem" => {
            let (request_id, invitation) =
                command.space_redeem().ok_or(ProtocolError::INVALID_INPUT)?;
            space::redeem(context, request_id, invitation).await?
        }
        "space_revoke" => {
            let (space_id, endpoint_id) =
                command.space_revoke().ok_or(ProtocolError::INVALID_INPUT)?;
            space::revoke(context, space_id, endpoint_id).await?
        }
        "private_relay_configure" => relay::private_configure(context, command).await?,
        "private_relay_disable" => relay::private_disable(context).await?,
        "private_relay_status" => relay::private_status(context).await?,
        "public_relay_configure" => {
            relay::public_configure(
                context,
                command
                    .public_relay_url()
                    .ok_or(ProtocolError::INVALID_INPUT)?,
            )
            .await?
        }
        "public_relay_disable" => relay::public_disable(context).await?,
        "public_relay_status" => relay::public_status(context).await?,
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
                ControlSyncView::new(if synchronized { vec![peer] } else { Vec::new() })
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
                ControlSyncView::new(if synchronized { vec![peer] } else { Vec::new() })
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
                EchoReplyView::new(
                    response.responder_endpoint_id(),
                    echoed,
                    response.duration_ms(),
                )
                .map_err(|_| ProtocolError::INTERNAL)?,
            )
        }
        "space_details_fetch" => CommandResult::space_details(
            context
                .handle
                .space_details(
                    command
                        .space_show_id()
                        .ok_or(ProtocolError::INVALID_INPUT)?,
                )
                .await
                .map_err(|_| ProtocolError::UNAVAILABLE)?
                .ok_or(ProtocolError::NOT_FOUND)?,
        ),
        "snapshot_stamp" => CommandResult::snapshot_stamp(
            context
                .handle
                .snapshot_stamp()
                .await
                .map_err(|_| ProtocolError::UNAVAILABLE)?,
        ),
        "snapshot_fetch" => CommandResult::snapshot(
            context
                .handle
                .snapshot()
                .await
                .map_err(|_| ProtocolError::UNAVAILABLE)?,
        ),
        "ui_start" => {
            let (host, port) = command.ui_binding().ok_or(ProtocolError::INVALID_INPUT)?;
            CommandResult::ui_status(
                context
                    .web
                    .as_ref()
                    .ok_or(ProtocolError::UNAVAILABLE)?
                    .start(host, port)
                    .await?,
            )
        }
        "ui_stop" => CommandResult::ui_status(match &context.web {
            Some(web) => web.stop().await,
            None => crate::api::UiStatusView::new(None),
        }),
        "ui_status" => CommandResult::ui_status(match &context.web {
            Some(web) => web.status().await,
            None => crate::api::UiStatusView::new(None),
        }),
        "graceful_shutdown" => CommandResult::shutting_down(),
        "ui_init" | "ui_password_set" | "ui_password_reset" | "session_revoke_all" => context
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

#[cfg(test)]
mod tests {
    use super::uses_post_execution_actor_revision;

    #[test]
    fn echo_response_uses_post_execution_actor_revision() {
        assert!(uses_post_execution_actor_revision("echo_call"));
    }
}
