//! Local operator commands on the services this Runtime hosts.

use ma2a_core::{ProtocolError, service::ServiceError};

use crate::api::{ApiError, Command, CommandResult, HostedCommand, MAX_HOSTED_LIST_PAGE};

use super::super::ConnectionContext;

pub(super) async fn execute(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let command = command
        .hosted_command()
        .ok_or_else(ApiError::invalid_input)?;
    let hosted = context.handle.hosted().ok_or_else(|| {
        ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "this Runtime does not host services",
        )
        .without_effects()
    })?;
    let result = match command {
        HostedCommand::Register(_, name, manifest) => {
            let (service, created) = hosted
                .register(name, (**manifest).clone())
                .await
                .map_err(refusal)?;
            CommandResult::service_registered(&service, created)
        }
        HostedCommand::Unregister(_, name) => {
            hosted.unregister(name).await.map_err(refusal)?;
            CommandResult::service_unregistered(name)
        }
        HostedCommand::Start(_, name) => {
            CommandResult::service_started(&hosted.start(name).await.map_err(refusal)?)
        }
        HostedCommand::Stop(_, name) => {
            CommandResult::service_stopped(&hosted.stop(name).await.map_err(refusal)?)
        }
        HostedCommand::List(after) => {
            let limit = u32::try_from(MAX_HOSTED_LIST_PAGE + 1).unwrap_or(u32::MAX);
            let (revision, services) =
                hosted.list((after.clone(), limit)).await.map_err(refusal)?;
            return Ok(CommandResult::hosted_services(services).at_revision(revision));
        }
        HostedCommand::Attach(_) | HostedCommand::Accept(..) => {
            return Err(ApiError::invalid_input());
        }
    };
    let revision = hosted.revision().await.map_err(refusal)?;
    Ok(result.at_revision(revision))
}

/// Maps a hosted-service refusal onto the local API error set.
const fn refusal(error: ServiceError) -> ApiError {
    match error {
        ServiceError::UnknownService => ApiError::new(ProtocolError::NOT_FOUND).without_effects(),
        ServiceError::Conflict => ApiError::with_remediation(
            ProtocolError::CONFLICT,
            "the service is started; stop it first",
        )
        .without_effects(),
        ServiceError::ConcurrencyExceeded => ApiError::with_remediation(
            ProtocolError::UNAVAILABLE,
            "at most 64 services may be registered; unregister one first",
        )
        .without_effects(),
        ServiceError::InvalidInput => ApiError::invalid_input().without_effects(),
        _ => ApiError::new(ProtocolError::UNAVAILABLE),
    }
}
