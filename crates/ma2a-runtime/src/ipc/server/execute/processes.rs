//! Local operator commands on the Processes this Runtime hosts.

use ma2a_core::{
    ProtocolError,
    service::{ProcessId, ProcessReason, ServiceError},
};

use crate::{
    api::{
        ApiError, Command, CommandResult, MAX_PROCESS_LIST_PAGE, ProcessCommand,
        ProcessDetailsView, ProcessView,
    },
    processes::ProcessManager,
};

use super::super::ConnectionContext;

pub(super) async fn execute(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let command = command
        .process_command()
        .ok_or_else(ApiError::invalid_input)?;
    let manager = context.handle.processes().ok_or_else(|| {
        ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "this Runtime does not host Processes",
        )
        .without_effects()
    })?;
    match *command {
        ProcessCommand::List(after) => list(manager, after).await,
        ProcessCommand::Inspect(id) => {
            let (record, details) = manager.details(None, id).await.map_err(refusal)?;
            let view =
                ProcessDetailsView::new(ProcessView::new(record), (details.start, details.log));
            Ok(CommandResult::process_details(view))
        }
        ProcessCommand::Terminate(_, id, grace_ms) => {
            let record = manager
                .terminate((None, id), (grace_ms, ProcessReason::Requested))
                .await
                .map_err(refusal)?;
            committed(
                manager,
                CommandResult::process_terminated(ProcessView::new(record)),
            )
            .await
        }
        ProcessCommand::Delete(_, id) => {
            // The archive's own record and revision: nothing after its commit, neither a log
            // left behind nor a later Store write, changes what the deletion answers.
            let deleted = manager.delete(None, id).await.map_err(refusal)?;
            let revision = deleted.revision();
            let view = ProcessView::new(deleted.into_value());
            Ok(CommandResult::process_deleted(view).at_revision(revision))
        }
    }
}

async fn list(
    manager: &ProcessManager,
    after: Option<ProcessId>,
) -> Result<CommandResult, ApiError> {
    let mut records = manager
        .records(None, (after, MAX_PROCESS_LIST_PAGE + 1))
        .await
        .map_err(refusal)?;
    let more = records.len() > MAX_PROCESS_LIST_PAGE;
    records.truncate(MAX_PROCESS_LIST_PAGE);
    let next = more
        .then(|| records.last().map(|record| record.id))
        .flatten();
    let views = records.into_iter().map(ProcessView::new).collect();
    CommandResult::processes(views, next)
}

/// Stamps a mutation with the Store revision its commit reached.
async fn committed(
    manager: &ProcessManager,
    result: CommandResult,
) -> Result<CommandResult, ApiError> {
    let revision = manager.revision().await.map_err(refusal)?;
    Ok(result.at_revision(revision))
}

/// Maps a Process refusal onto the local API error set.
const fn refusal(error: ServiceError) -> ApiError {
    match error {
        ServiceError::UnknownProcess => ApiError::new(ProtocolError::NOT_FOUND).without_effects(),
        ServiceError::ProcessActive => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "the Process has not ended; terminate it first",
        )
        .without_effects(),
        ServiceError::NotRunning => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "the Process never started; it is concluded when the Runtime reconciles it",
        )
        .without_effects(),
        _ => ApiError::new(ProtocolError::UNAVAILABLE),
    }
}
