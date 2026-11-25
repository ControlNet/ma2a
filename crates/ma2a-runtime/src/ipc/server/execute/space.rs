use iroh_tickets::Ticket as _;
use ma2a_core::{
    EndpointId, ProtocolError, RequestId, SignedInviteTicket, SpaceId, default_member_label,
};

use crate::{SpaceDepartureErrorCode, api::ApiError, api::CommandResult};

use super::super::ConnectionContext;
#[path = "space_invite.rs"]
mod invite;
pub(super) use invite::invite;

pub(super) async fn create(
    context: &ConnectionContext,
    name: &str,
) -> Result<CommandResult, ApiError> {
    let committed = context
        .handle
        .create_owned_space_committed(name.to_owned())
        .await
        .map_err(|error| committed_runtime_error(&error))?;
    Ok(
        CommandResult::space_created(committed_space(committed.chain())?)
            .at_revision(committed.revision()),
    )
}

fn committed_space(chain: &ma2a_core::SpaceChain) -> Result<crate::api::SpaceView, ProtocolError> {
    let fallback = crate::api::encode_hex(chain.space_id().as_bytes());
    crate::api::SpaceView::new(
        chain.space_id(),
        chain.genesis().genesis().name().unwrap_or(&fallback),
        u32::try_from(chain.members().len()).map_err(|_| ProtocolError::INTERNAL)?,
    )
    .map_err(|_| ProtocolError::INTERNAL)
}

pub(super) async fn list(context: &ConnectionContext) -> Result<CommandResult, ProtocolError> {
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| ProtocolError::UNAVAILABLE)?;
    CommandResult::spaces(
        snapshot
            .spaces()
            .iter()
            .map(|space| space.to_space_view().map_err(|_| ProtocolError::INTERNAL))
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map(|result| result.at_revision(snapshot.revision()))
    .map_err(|_| ProtocolError::INTERNAL)
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
        .and_then(|space| space.to_space_view().ok())
        .map(|space| CommandResult::space(space).at_revision(snapshot.revision()))
        .ok_or(ProtocolError::NOT_FOUND)
}

pub(super) async fn leave(
    context: &ConnectionContext,
    space_id: SpaceId,
    request_id: RequestId,
) -> Result<CommandResult, ApiError> {
    // Identity is read before departure, because afterwards this Runtime is no
    // longer a member and can no longer project the Space it just left. Only the
    // Space's stable identity is carried forward; membership state would be stale.
    let snapshot = context
        .handle
        .snapshot()
        .await
        .map_err(|_| without_effects(ProtocolError::UNAVAILABLE))?;
    let space = snapshot
        .spaces()
        .iter()
        .find(|space| space.id() == space_id)
        .cloned()
        .ok_or_else(|| without_effects(ProtocolError::NOT_FOUND))?;
    let identity = crate::api::SpaceIdentityView::new(space.space_id(), space.name())
        .map_err(|_| without_effects(ProtocolError::INTERNAL))?;
    let revision = context
        .handle
        .leave_space(space_id, request_id)
        .await
        .map_err(|error| {
            let result = departure_api_error(error.code());
            error
                .committed_revision()
                .map_or(result, |revision| result.after_commit(revision))
        })?;
    Ok(CommandResult::space_left(identity).at_revision(revision))
}

fn departure_api_error(code: SpaceDepartureErrorCode) -> ApiError {
    if code == SpaceDepartureErrorCode::OWNER_CANNOT_LEAVE {
        ApiError::with_remediation(
            ProtocolError::UNAUTHORIZED,
            "Space owner cannot leave its own Space",
        )
        .without_effects()
    } else if code == SpaceDepartureErrorCode::NOT_A_MEMBER {
        ApiError::new(ProtocolError::NOT_FOUND)
    } else if code == SpaceDepartureErrorCode::UNREACHABLE {
        ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "departure exchange did not complete; authority outcome is unknown",
        )
    } else if code == SpaceDepartureErrorCode::IN_FLIGHT {
        ApiError::new(ProtocolError::CONFLICT).without_effects()
    } else if code == SpaceDepartureErrorCode::REJECTED {
        ApiError::new(ProtocolError::CONFLICT)
    } else {
        ApiError::new(ProtocolError::INTERNAL)
    }
}

pub(super) async fn redeem(
    context: &ConnectionContext,
    request_id: RequestId,
    invitation: &str,
) -> Result<CommandResult, ApiError> {
    let ticket = SignedInviteTicket::decode_string(invitation.trim()).map_err(|_| not_run())?;
    // The joining member is labelled by its own Endpoint identity, never by the
    // Space it is joining and never by a placeholder standing in for a person.
    let local = context
        .handle
        .status()
        .await
        .map_err(|_| without_effects(ProtocolError::UNAVAILABLE))?
        .endpoint_id();
    let committed = context
        .handle
        .clone()
        .redeem_enrollment(crate::EnrollmentAttempt::new(
            ticket,
            request_id,
            default_member_label(local),
        ))
        .await
        .map_err(|error| {
            let api = ApiError::new(enrollment_protocol_error(&error));
            if error.code() == crate::EnrollmentErrorCode::BUSY {
                return api.without_effects();
            }
            error
                .committed_revision()
                .map_or(api, |revision| api.after_commit(revision))
        })?;
    Ok(
        CommandResult::space_redeemed(committed_space(committed.chain())?)
            .at_revision(committed.revision()),
    )
}

pub(super) async fn revoke(
    context: &ConnectionContext,
    space_id: SpaceId,
    endpoint_id: EndpointId,
) -> Result<CommandResult, ApiError> {
    #[cfg(test)]
    context.mutation_hooks.pause_revoke().await;
    let removed = context
        .handle
        .revoke_owned_space_member(space_id, endpoint_id)
        .await
        .map_err(|error| {
            if error.code() == crate::error::RuntimeErrorCode::SPACE_OWNER_CANNOT_BE_REMOVED {
                ApiError::with_remediation(
                    ProtocolError::UNAUTHORIZED,
                    "Space owner cannot be removed in Phase 1",
                )
                .without_effects()
            } else {
                authority_error(&error)
            }
        })?;
    Ok(
        CommandResult::space_revoked(committed_space(&removed.chain)?)
            .at_revision(removed.revision),
    )
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
    } else if error.code() == crate::EnrollmentErrorCode::BUSY {
        ProtocolError::UNAVAILABLE
    } else {
        ProtocolError::INTERNAL
    }
}

/// A refusal made before the request handed anything to the Runtime, so it had no effect.
const fn without_effects(error: ProtocolError) -> ApiError {
    ApiError::new(error).without_effects()
}

/// Invalid input found before the request ran.
const fn not_run() -> ApiError {
    ApiError::invalid_input().without_effects()
}

/// Maps a failure of a change signed with a Space's authority. What the Store refused before
/// writing anything has no effect: a Space or member it does not hold, a Space this Runtime
/// does not own, or a bound the change would pass.
fn authority_error(error: &crate::RuntimeError) -> ApiError {
    if error.is_space_not_found() {
        without_effects(ProtocolError::NOT_FOUND)
    } else if error.is_not_space_owner() {
        without_effects(ProtocolError::UNAUTHORIZED)
    } else if error.is_request_refusal() {
        without_effects(ProtocolError::CONFLICT)
    } else {
        committed_runtime_error(error)
    }
}

fn committed_runtime_error(error: &crate::RuntimeError) -> ApiError {
    let api = ApiError::new(ProtocolError::INTERNAL);
    error
        .committed_revision()
        .map_or(api, |revision| api.after_commit(revision))
}
