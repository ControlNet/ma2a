//! Service grants, local exposure, and operation status.

use ma2a_core::{Capability, ProtocolError, SpaceId};
use ma2a_services::OperationEnd;
use ma2a_store::{ExposureRecord, FileRootExposure, LocalExposure, StoreError};

use crate::{
    api::{
        ApiError, Command, CommandResult, ExposureSpec, OperationEndView, OperationIdentity,
        OperationRecordView, OperationsView, ServiceExposureView, SpaceMemberView,
    },
    store::{GrantRefusal, SERVICE_CAPABILITIES},
    stream_host::operations::{Direction, OperationKey, OperationRecord},
};

use super::super::ConnectionContext;

/// Most exposures one Runtime holds across all Spaces, the local API collection bound.
const MAX_EXPOSURES: usize = 256;

pub(super) async fn grant(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let (member, services) = command.space_grant().ok_or_else(ApiError::invalid_input)?;
    let granted = context
        .handle
        .grant_owned_space_member(member, services)
        .await
        .map_err(|error| runtime_error(&error))?
        .map_err(|refusal| match refusal {
            GrantRefusal::NotFound => ApiError::new(ProtocolError::NOT_FOUND).without_effects(),
            GrantRefusal::NotOwner => ApiError::with_remediation(
                ProtocolError::UNAUTHORIZED,
                "only the Space owner can change member grants",
            )
            .without_effects(),
        })?;
    let capabilities = granted.member.capabilities();
    let view = SpaceMemberView::new(
        granted.member.endpoint_id(),
        granted.member.label(),
        capabilities.allows(Capability::ECHO),
        capabilities.allows(Capability::PRIVATE_RELAY_PROVIDER),
    )
    .map_err(|error| error.after_commit(granted.revision))?
    .with_services(SERVICE_CAPABILITIES.map(|capability| capabilities.allows(capability)));
    Ok(CommandResult::space_granted(view).at_revision(granted.revision))
}

pub(super) async fn expose(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let (space_id, spec) = command
        .service_exposure()
        .ok_or_else(ApiError::invalid_input)?;
    let exposure = local_exposure(spec).await?;
    let existing = context
        .handle
        .service_exposures()
        .await
        .map_err(|_| ApiError::new(ProtocolError::UNAVAILABLE).without_effects())?;
    let replaces = existing
        .value()
        .iter()
        .any(|record| record.space_id == space_id && same_exposure(&record.exposure, spec));
    if !replaces && existing.value().len() >= MAX_EXPOSURES {
        return Err(ApiError::with_remediation(
            ProtocolError::INVALID_INPUT,
            "at most 256 exposures; remove one first",
        )
        .without_effects());
    }
    let committed = context
        .handle
        .expose_service(space_id, exposure.clone())
        .await
        .map_err(|error| runtime_error(&error))?
        .map_err(|error| match error {
            StoreError::SpaceNotFound => ApiError::new(ProtocolError::NOT_FOUND).without_effects(),
            StoreError::Protocol(ProtocolError::CONFLICT) => ApiError::with_remediation(
                ProtocolError::CONFLICT,
                "this root name already names a different directory",
            )
            .without_effects(),
            _ => ApiError::new(ProtocolError::INTERNAL),
        })?;
    let view = exposure_view(space_id, &exposure)
        .map_err(|error| error.after_commit(committed.revision()))?;
    Ok(CommandResult::service_exposed(view).at_revision(committed.revision()))
}

pub(super) async fn unexpose(
    context: &ConnectionContext,
    command: &Command,
) -> Result<CommandResult, ApiError> {
    let (space_id, spec) = command
        .service_exposure()
        .ok_or_else(ApiError::invalid_input)?;
    let existing = context
        .handle
        .service_exposures()
        .await
        .map_err(|_| ApiError::new(ProtocolError::UNAVAILABLE).without_effects())?;
    let record = existing
        .into_value()
        .into_iter()
        .find(|record| record.space_id == space_id && same_exposure(&record.exposure, spec))
        .ok_or_else(|| ApiError::new(ProtocolError::NOT_FOUND).without_effects())?;
    let committed = context
        .handle
        .unexpose_service(space_id, record.exposure.clone())
        .await
        .map_err(|error| runtime_error(&error))?
        .map_err(|_| ApiError::new(ProtocolError::INTERNAL))?;
    let view = exposure_view(space_id, &record.exposure)
        .map_err(|error| error.after_commit(committed.revision()))?;
    Ok(CommandResult::service_unexposed(view).at_revision(committed.revision()))
}

pub(super) async fn list(context: &ConnectionContext) -> Result<CommandResult, ApiError> {
    let committed = context
        .handle
        .service_exposures()
        .await
        .map_err(|_| ApiError::new(ProtocolError::UNAVAILABLE))?;
    let revision = committed.revision();
    let views = committed
        .into_value()
        .iter()
        .map(
            |ExposureRecord {
                 space_id, exposure, ..
             }| exposure_view(*space_id, exposure),
        )
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CommandResult::service_exposures(views).at_revision(revision))
}

pub(super) fn operations(context: &ConnectionContext) -> CommandResult {
    let snapshot = context.handle.operations().snapshot();
    let active = snapshot
        .active
        .iter()
        .map(|(operation, elapsed_ms)| (identity(&operation.key), *elapsed_ms))
        .collect();
    let recent = snapshot.recent.iter().map(record_view).collect();
    CommandResult::operations(OperationsView::new(active, recent))
}

fn identity(key: &OperationKey) -> OperationIdentity {
    OperationIdentity::new(
        key.request_id,
        key.peer,
        (
            key.kind,
            key.operation,
            key.direction == Direction::Outbound,
        ),
    )
}

fn record_view(record: &OperationRecord) -> OperationRecordView {
    let end = match record.end {
        OperationEnd::Completed => OperationEndView::completed(),
        OperationEnd::Rejected(error) => OperationEndView::rejected(error),
        OperationEnd::Failed(error) => OperationEndView::failed(error),
    };
    OperationRecordView::new(
        identity(&record.key),
        end,
        (record.bytes_in, record.bytes_out),
    )
    .with_duration_ms(record.duration_ms)
}

/// Resolves a requested exposure; an exported root must be an existing absolute directory
/// and is stored canonicalized, so one root name always names exactly one directory.
async fn local_exposure(spec: &ExposureSpec) -> Result<LocalExposure, ApiError> {
    let refused = || ApiError::invalid_input().without_effects();
    Ok(match spec {
        ExposureSpec::FilesRoot {
            root,
            local: Some((path, access)),
        } => {
            let path = std::path::Path::new(path.as_str());
            if !path.is_absolute() {
                return Err(ApiError::with_remediation(
                    ProtocolError::INVALID_INPUT,
                    "an exported root must be an absolute directory path",
                )
                .without_effects());
            }
            let canonical = tokio::fs::canonicalize(path).await.map_err(|_| {
                ApiError::with_remediation(
                    ProtocolError::NOT_FOUND,
                    "the directory to export does not exist",
                )
                .without_effects()
            })?;
            let metadata = tokio::fs::metadata(&canonical)
                .await
                .map_err(|_| refused())?;
            if !metadata.is_dir() {
                return Err(refused());
            }
            let canonical = canonical
                .into_os_string()
                .into_string()
                .map_err(|_| refused())?;
            LocalExposure::FilesRoot(FileRootExposure::new(root.clone(), canonical, *access))
        }
        ExposureSpec::FilesRoot { local: None, .. } => return Err(refused()),
        ExposureSpec::Exec => LocalExposure::Exec,
        ExposureSpec::Shell => LocalExposure::Shell,
        ExposureSpec::Process => LocalExposure::Process,
        ExposureSpec::Agent(agent) => LocalExposure::Agent(agent.clone()),
        ExposureSpec::Proxy(rule) => LocalExposure::Proxy(rule.clone()),
    })
}

/// Returns whether a stored exposure is the one a command names.
fn same_exposure(exposure: &LocalExposure, spec: &ExposureSpec) -> bool {
    match (exposure, spec) {
        (LocalExposure::FilesRoot(stored), ExposureSpec::FilesRoot { root, .. }) => {
            stored.name() == root
        }
        (LocalExposure::Exec, ExposureSpec::Exec)
        | (LocalExposure::Shell, ExposureSpec::Shell)
        | (LocalExposure::Process, ExposureSpec::Process) => true,
        (LocalExposure::Proxy(stored), ExposureSpec::Proxy(rule)) => stored == rule,
        (LocalExposure::Agent(stored), ExposureSpec::Agent(agent)) => stored == agent,
        _ => false,
    }
}

fn exposure_view(
    space_id: SpaceId,
    exposure: &LocalExposure,
) -> Result<ServiceExposureView, ApiError> {
    Ok(match exposure {
        LocalExposure::FilesRoot(root) => ServiceExposureView::files_root(
            space_id,
            (root.name().as_str(), root.path()),
            root.access(),
        )?,
        LocalExposure::Exec => ServiceExposureView::exec(space_id),
        LocalExposure::Shell => ServiceExposureView::shell(space_id),
        LocalExposure::Process => ServiceExposureView::process(space_id),
        LocalExposure::Proxy(rule) => ServiceExposureView::proxy(space_id, rule.clone()),
        LocalExposure::Agent(agent) => ServiceExposureView::agent(space_id, agent.clone()),
    })
}

fn runtime_error(error: &crate::RuntimeError) -> ApiError {
    let api = ApiError::new(ProtocolError::INTERNAL);
    error
        .committed_revision()
        .map_or(api, |revision| api.after_commit(revision))
}
