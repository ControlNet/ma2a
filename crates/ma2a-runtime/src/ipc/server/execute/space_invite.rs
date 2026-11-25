//! `space_invite`: one invitation, with its ticket written to a file only this account reads.

use iroh_tickets::Ticket as _;
use ma2a_core::ProtocolError;

use super::{authority_error, committed_space, not_run, without_effects};
use crate::api::{ApiError, CommandResult};

use super::super::super::ConnectionContext;
#[path = "invite_file.rs"]
mod invite_file;

pub(in super::super) async fn invite(
    context: &ConnectionContext,
    command: &crate::api::Command,
) -> Result<CommandResult, ApiError> {
    let (space_id, ttl_ms, output_path) = command.space_invite().ok_or_else(not_run)?;
    let entropy =
        ma2a_core::InviteEntropy::random().map_err(|_| without_effects(ProtocolError::INTERNAL))?;
    let creation =
        crate::EnrollmentCreation::new(space_id, ttl_ms, entropy).map_err(|_| not_run())?;
    // Refused before the invitation is committed, so a bad path never costs a revision, and
    // the same request can run once the path is fixed. Only valid input gets this far, so an
    // invalid request never creates a staging file.
    let output_path = output_path.to_owned();
    let destination =
        tokio::task::spawn_blocking(move || invite_file::Destination::open(&output_path))
            .await
            .unwrap_or(Err(ProtocolError::INTERNAL))
            .map_err(without_effects)?;
    let created = context
        .handle
        .create_enrollment_invite_committed(creation)
        .await
        .map_err(|error| authority_error(&error))?;
    let encoded = created.ticket().encode_string().into_bytes();
    let written = if injected_write_failure(context) {
        Err(ProtocolError::INTERNAL)
    } else {
        tokio::task::spawn_blocking(move || destination.write(&encoded))
            .await
            .unwrap_or(Err(ProtocolError::INTERNAL))
    };
    written.map_err(|error| ApiError::new(error).after_commit(created.revision()))?;
    Ok(
        CommandResult::space_invitation_created(committed_space(created.chain())?)
            .at_revision(created.revision()),
    )
}

/// Deterministic fault point: the ticket file fails to be written after the commit.
#[cfg(test)]
fn injected_write_failure(context: &ConnectionContext) -> bool {
    context.mutation_hooks.take_invite_write_failure()
}

#[cfg(not(test))]
const fn injected_write_failure(_context: &ConnectionContext) -> bool {
    false
}
