use iroh_tickets::Ticket as _;
use ma2a_core::{EndpointId, ProtocolError, RequestId, SignedInviteTicket, SpaceId};

use crate::api::CommandResult;

use super::super::ConnectionContext;

pub(super) async fn create(
    context: &ConnectionContext,
    name: &str,
) -> Result<CommandResult, ProtocolError> {
    let space_id = context
        .handle
        .create_owned_space(name.to_owned())
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(CommandResult::space_created(
        crate::api::SpaceView::new(space_id, name, 1).map_err(|_| ProtocolError::INTERNAL)?,
    ))
}

pub(super) async fn invite(
    context: &ConnectionContext,
    command: &crate::api::Command,
) -> Result<CommandResult, ProtocolError> {
    let (space_id, ttl_ms, output_path) =
        command.space_invite().ok_or(ProtocolError::INVALID_INPUT)?;
    let ticket = context
        .handle
        .create_enrollment_invite(
            crate::EnrollmentCreation::new(
                space_id,
                ttl_ms,
                ma2a_core::InviteEntropy::random().map_err(|_| ProtocolError::INTERNAL)?,
            )
            .map_err(|_| ProtocolError::INVALID_INPUT)?,
        )
        .await
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    write_owner_only(output_path, ticket.encode_string().as_bytes())?;
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    snapshot
        .spaces()
        .iter()
        .find(|space| space.id() == space_id)
        .cloned()
        .map(CommandResult::space_invitation_created)
        .ok_or(ProtocolError::INTERNAL)
}

#[cfg(unix)]
fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    file.write_all(contents)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| ProtocolError::INTERNAL)
}

#[cfg(not(unix))]
fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    file.write_all(contents)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| ProtocolError::INTERNAL)
}

pub(super) async fn list(context: &ConnectionContext) -> Result<CommandResult, ProtocolError> {
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    CommandResult::spaces(snapshot.spaces().to_vec()).map_err(|_| ProtocolError::INTERNAL)
}

pub(super) async fn show(
    context: &ConnectionContext,
    space_id: SpaceId,
) -> Result<CommandResult, ProtocolError> {
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    snapshot
        .spaces()
        .iter()
        .find(|space| space.id() == space_id)
        .cloned()
        .map(CommandResult::space)
        .ok_or(ProtocolError::INVALID_INPUT)
}

pub(super) async fn redeem(
    context: &ConnectionContext,
    request_id: RequestId,
    invitation: &str,
) -> Result<CommandResult, ProtocolError> {
    let ticket = SignedInviteTicket::decode_string(invitation.trim())
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let space_id = ticket.space_id();
    context
        .handle
        .clone()
        .redeem_enrollment(crate::EnrollmentAttempt::new(
            ticket,
            request_id,
            "local-endpoint".to_owned(),
        ))
        .await
        .map_err(|error| enrollment_protocol_error(&error))?;
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    snapshot
        .spaces()
        .iter()
        .find(|space| space.id() == space_id)
        .cloned()
        .map(CommandResult::space_redeemed)
        .ok_or(ProtocolError::INTERNAL)
}

pub(super) async fn revoke(
    context: &ConnectionContext,
    space_id: SpaceId,
    endpoint_id: EndpointId,
) -> Result<CommandResult, ProtocolError> {
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    let mut space = snapshot
        .spaces()
        .iter()
        .find(|space| space.id() == space_id)
        .cloned()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    context
        .handle
        .revoke_owned_space_member(space_id, endpoint_id)
        .await
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    space.member_count -= 1;
    Ok(CommandResult::space_revoked(space))
}

fn enrollment_protocol_error(error: &crate::EnrollmentError) -> ProtocolError {
    if error.code() == crate::EnrollmentErrorCode::INVALID_TICKET {
        ProtocolError::INVALID_INPUT
    } else if error.code() == crate::EnrollmentErrorCode::EXPIRED {
        ProtocolError::EXPIRED
    } else if error.code() == crate::EnrollmentErrorCode::CANCELLED {
        ProtocolError::NOT_FOUND
    } else if error.code() == crate::EnrollmentErrorCode::CONFLICT {
        ProtocolError::CONFLICT
    } else {
        ProtocolError::INTERNAL
    }
}
