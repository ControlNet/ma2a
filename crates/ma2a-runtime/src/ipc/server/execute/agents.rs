//! Local operator commands on the Agents this Runtime hosts and their tasks.
//!
//! The local operator sees every task; the tasks it submits are created as this Endpoint.

use ma2a_core::{
    ProtocolError,
    service::{AgentId, AgentSubmit, AgentTaskReason, ServiceError},
};

use crate::{
    agents::AgentManager,
    api::{
        AgentCommand, AgentView, ApiError, Command, CommandResult, MAX_AGENT_EVENT_PAGE,
        MAX_AGENT_LIST_PAGE, MAX_AGENT_TASK_PAGE,
    },
};

use super::super::ConnectionContext;

/// The local operator: every task, of every Agent.
const OPERATOR: (Option<ma2a_core::EndpointId>, Option<&AgentId>) = (None, None);

pub(super) async fn execute(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let command = command
        .agent_command()
        .ok_or_else(ApiError::invalid_input)?;
    let manager = context.handle.agents().ok_or_else(|| {
        ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "this Runtime does not host Agents",
        )
        .without_effects()
    })?;
    let result = match command {
        AgentCommand::Define(_, spec) => {
            // Answered at the revision of its own commit, never a later one.
            let ((record, created), revision) =
                manager.define((**spec).clone()).await.map_err(active)?;
            return Ok(
                CommandResult::agent_defined(&AgentView::new(record, 0), created)
                    .at_revision(revision),
            );
        }
        AgentCommand::Undefine(_, agent) => {
            let revision = manager.undefine(agent).await.map_err(active)?;
            return Ok(CommandResult::agent_undefined(agent.as_str()).at_revision(revision));
        }
        AgentCommand::Start(_, agent) => {
            let record = manager.start(agent).await.map_err(refusal)?;
            CommandResult::agent_started(&view(manager, record).await?)
        }
        AgentCommand::Stop(_, agent) => {
            let record = manager.stop(agent).await.map_err(refusal)?;
            CommandResult::agent_stopped(&view(manager, record).await?)
        }
        AgentCommand::List(after) => return list(manager, after.as_ref()).await,
        AgentCommand::Tasks(agent, after) => {
            let limit = u32::try_from(MAX_AGENT_TASK_PAGE + 1).unwrap_or(u32::MAX);
            let (revision, tasks) = manager
                .tasks((None, agent.clone()), (*after, limit))
                .await
                .map_err(refusal)?;
            let summaries = tasks.into_iter().map(|task| task.summary).collect();
            return Ok(CommandResult::agent_tasks(summaries).at_revision(revision));
        }
        AgentCommand::Inspect(task) => {
            let details = manager.details(OPERATOR, *task).await.map_err(refusal)?;
            return Ok(CommandResult::agent_task(&details));
        }
        AgentCommand::Events(task, after) => {
            let limit = u32::try_from(MAX_AGENT_EVENT_PAGE + 1).unwrap_or(u32::MAX);
            let events = manager
                .events(OPERATOR, (*task, *after, limit))
                .await
                .map_err(refusal)?;
            return Ok(CommandResult::agent_task_events(events));
        }
        AgentCommand::Artifact(task, name, offset) => {
            let (artifact, bytes) = manager
                .read_artifact(OPERATOR, ((*task, name), *offset))
                .await
                .map_err(refusal)?;
            return Ok(CommandResult::agent_task_artifact(
                &artifact,
                (*offset, &bytes),
            ));
        }
        AgentCommand::Submit(request_id, agent, message) => {
            let submit = AgentSubmit::new(agent.clone(), message.clone(), None);
            let (record, existing) = manager
                .submit((manager.local(), *request_id), &submit)
                .await
                .map_err(refusal)?;
            CommandResult::agent_task_submitted(&record.summary, existing)
        }
        AgentCommand::Send(_, task, message) => {
            let record = manager
                .send(OPERATOR, (*task, message.clone()))
                .await
                .map_err(refusal)?;
            CommandResult::agent_task_sent(&record.summary)
        }
        AgentCommand::Cancel(_, task) => {
            let record = manager
                .request_cancel(OPERATOR, (*task, AgentTaskReason::Requested))
                .await
                .map_err(refusal)?;
            CommandResult::agent_task_cancelled(&record.summary)
        }
    };
    let revision = manager.revision().await.map_err(refusal)?;
    Ok(result.at_revision(revision))
}

/// One page of Agents in name order after `after`.
async fn list(manager: &AgentManager, after: Option<&AgentId>) -> Result<CommandResult, ApiError> {
    let (revision, agents) = manager.list().await.map_err(refusal)?;
    let mut views: Vec<AgentView> = agents
        .into_iter()
        .filter(|(record, _)| {
            after.is_none_or(|after| record.definition.id.as_str() > after.as_str())
        })
        .map(|(record, live)| AgentView::new(record, live))
        .collect();
    views.truncate(MAX_AGENT_LIST_PAGE + 1);
    Ok(CommandResult::agents(views).at_revision(revision))
}

/// One Agent's view with its current number of unfinished tasks.
async fn view(
    manager: &AgentManager,
    record: ma2a_store::AgentRecord,
) -> Result<AgentView, ApiError> {
    let (_, agents) = manager.list().await.map_err(refusal)?;
    let live = agents
        .into_iter()
        .find(|(listed, _)| listed.definition.id == record.definition.id)
        .map_or(0, |(_, live)| live);
    Ok(AgentView::new(record, live))
}

/// Maps a definition change's refusal: a conflict there means the Agent is started.
const fn active(error: ServiceError) -> ApiError {
    match error {
        ServiceError::Conflict => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "the Agent is started; stop it first",
        )
        .without_effects(),
        other => refusal(other),
    }
}

/// Maps an Agent refusal onto the local API error set.
const fn refusal(error: ServiceError) -> ApiError {
    match error {
        ServiceError::UnknownAgent | ServiceError::UnknownTask | ServiceError::UnknownArtifact => {
            ApiError::new(ProtocolError::NOT_FOUND).without_effects()
        }
        ServiceError::AgentUnavailable => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "the Agent is not started; start it first",
        )
        .without_effects(),
        ServiceError::TaskEnded => {
            ApiError::with_remediation(ProtocolError::CONFLICT, "the task has already ended")
                .without_effects()
        }
        ServiceError::Conflict => ApiError::new(ProtocolError::CONFLICT).without_effects(),
        ServiceError::ConcurrencyExceeded => ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "a task bound is reached; retry once tasks end",
        )
        .without_effects(),
        ServiceError::InvalidInput => ApiError::invalid_input().without_effects(),
        _ => ApiError::new(ProtocolError::UNAVAILABLE),
    }
}
