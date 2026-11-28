//! Store-thread handling of streaming service authorization and upload staging.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use ma2a_core::SpaceId;
use ma2a_core::{
    AuthorizationEndpoints, EndpointId, RequestId,
    service::{ServiceAccess, ServiceAuthorizationRequest, ServiceError, authorize_service},
};
use ma2a_store::{
    Committed, ControlSpaceState, ExposureRecord, LocalExposure, Repository, StoreError,
    TransferStaging,
};
use tokio::sync::oneshot;

use super::{GoDispatchLease, StoreBackend};
use crate::error::RuntimeError;

/// What a successful authorization lets the host touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServiceGrant {
    /// Local canonical directory of the exported root named by the access, if any.
    pub(crate) root: Option<PathBuf>,
}

/// One Process read or step, run on the store thread against the repository.
pub(crate) type ProcessJob = Box<dyn FnOnce(&mut Repository) + Send>;

/// Upper bound on holding the Store thread while the daemon writes `go` to a local supervisor.
const GO_DISPATCH_HOLD: Duration = Duration::from_secs(6);

pub(crate) enum ServiceStoreCommand {
    Authorize {
        endpoints: (EndpointId, EndpointId),
        access: ServiceAccess,
        reply: oneshot::Sender<Result<ServiceGrant, ServiceError>>,
    },
    AuthorizeProcessGo {
        endpoints: (EndpointId, EndpointId),
        lease: GoDispatchLease,
        reply: oneshot::Sender<Result<(), ServiceError>>,
    },
    #[cfg(all(test, unix))]
    SetGoDispatchHold {
        hold: Duration,
        reply: oneshot::Sender<()>,
    },
    RecordStaging {
        path: String,
        request_id: RequestId,
        created_at_ms: u64,
        reply: oneshot::Sender<Result<(), ServiceError>>,
    },
    ForgetStaging {
        path: String,
        reply: oneshot::Sender<()>,
    },
    SweepStaging(oneshot::Sender<Result<usize, RuntimeError>>),
    Grant {
        request: super::OwnedMemberGrant,
        reply: oneshot::Sender<super::GrantResult>,
    },
    Expose {
        space_id: SpaceId,
        exposure: LocalExposure,
        reply: oneshot::Sender<Result<Committed<()>, StoreError>>,
    },
    Unexpose {
        space_id: SpaceId,
        exposure: LocalExposure,
        reply: oneshot::Sender<Result<Committed<bool>, StoreError>>,
    },
    Exposures(oneshot::Sender<Result<Committed<Vec<ExposureRecord>>, StoreError>>),
    Process(ProcessJob),
}

/// Authorizes one access for `peer` against this Runtime inside one complete shared Space.
///
/// Every input is read from durable state at call time, so a revocation, departure, or
/// unexposure applies to the very next stream.
pub(crate) fn authorize(
    repository: &Repository,
    (local, peer): (EndpointId, EndpointId),
    access: &ServiceAccess,
) -> Result<ServiceGrant, ServiceError> {
    let shared = repository
        .control_spaces_between(local, peer)
        .map_err(|_| ServiceError::Unavailable)?;
    let views = shared
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
    let exposures = repository
        .service_exposure_views()
        .map_err(|_| ServiceError::Unavailable)?;
    let request = ServiceAuthorizationRequest::new(
        AuthorizationEndpoints::new(peer, local),
        access,
        &exposures,
    );
    authorize_service(&request, &views).map_err(|_| ServiceError::Unauthorized)?;
    let root = match access {
        ServiceAccess::FilesRead(root) | ServiceAccess::FilesWrite(root) => Some(
            repository
                .file_root_path(root)
                .map_err(|_| ServiceError::Unavailable)?
                .map(PathBuf::from)
                .ok_or(ServiceError::Unauthorized)?,
        ),
        _ => None,
    };
    Ok(ServiceGrant { root })
}

/// Returns whether a recorded path names a staging file this Runtime could have created.
fn is_staging_file(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".ma2a-upload-"))
        .and_then(|rest| rest.strip_suffix(".part"))
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

impl StoreBackend {
    pub(super) fn dispatch_services(&mut self, command: ServiceStoreCommand) {
        match command {
            ServiceStoreCommand::Authorize {
                endpoints,
                access,
                reply,
            } => drop(reply.send(authorize(&self.repository, endpoints, &access))),
            ServiceStoreCommand::AuthorizeProcessGo {
                endpoints,
                lease,
                reply,
            } => self.authorize_process_go(endpoints, (&lease, reply)),
            #[cfg(all(test, unix))]
            ServiceStoreCommand::SetGoDispatchHold { hold, reply } => {
                self.go_dispatch_hold = Some(hold);
                let _sent = reply.send(());
            }
            ServiceStoreCommand::RecordStaging {
                path,
                request_id,
                created_at_ms,
                reply,
            } => drop(
                reply.send(
                    self.repository
                        .record_transfer_staging(TransferStaging::new(
                            &path,
                            request_id,
                            created_at_ms,
                        ))
                        .map_err(|_| ServiceError::Unavailable),
                ),
            ),
            ServiceStoreCommand::ForgetStaging { path, reply } => {
                let _forgotten = self.repository.remove_transfer_staging(&path);
                let _sent = reply.send(());
            }
            ServiceStoreCommand::SweepStaging(reply) => drop(reply.send(self.sweep_staging())),
            ServiceStoreCommand::Grant { request, reply } => {
                drop(reply.send(self.grant_owned_space_member(request)));
            }
            ServiceStoreCommand::Expose {
                space_id,
                exposure,
                reply,
            } => drop(reply.send(self.repository.expose_service(space_id, &exposure))),
            ServiceStoreCommand::Unexpose {
                space_id,
                exposure,
                reply,
            } => drop(reply.send(self.repository.unexpose_service(space_id, &exposure))),
            ServiceStoreCommand::Exposures(reply) => {
                drop(reply.send(self.repository.service_exposures()));
            }
            ServiceStoreCommand::Process(job) => job(&mut self.repository),
        }
    }

    /// Authorizes `Process` access and holds the Store thread until the caller writes `go`
    /// or `lease` expires, ordering `go` dispatch with authorization withdrawals.
    fn authorize_process_go(
        &self,
        endpoints: (EndpointId, EndpointId),
        (lease, reply): (&GoDispatchLease, oneshot::Sender<Result<(), ServiceError>>),
    ) {
        let decision = authorize(&self.repository, endpoints, &ServiceAccess::Process).map(|_| ());
        let ok = decision.is_ok();
        if reply.send(decision).is_err() || !ok {
            return;
        }
        #[cfg(all(test, unix))]
        let hold = self.go_dispatch_hold.unwrap_or(GO_DISPATCH_HOLD);
        #[cfg(not(all(test, unix)))]
        let hold = GO_DISPATCH_HOLD;
        lease.wait_until(Instant::now() + hold);
    }

    /// Removes every recorded staging file and its record; run before serving any stream.
    fn sweep_staging(&mut self) -> Result<usize, RuntimeError> {
        let paths = self.repository.transfer_staging_paths()?;
        for path in &paths {
            if is_staging_file(path) {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => continue,
                }
            }
            self.repository.remove_transfer_staging(path)?;
        }
        Ok(paths.len())
    }
}

#[cfg(test)]
mod tests {
    use super::is_staging_file;

    #[test]
    fn only_generated_staging_names_are_ever_swept() {
        assert!(is_staging_file("/srv/docs/.ma2a-upload-00ff.part"));
        assert!(!is_staging_file("/srv/docs/report.part"));
        assert!(!is_staging_file("/srv/docs/.ma2a-upload-00ff"));
        assert!(!is_staging_file("/"));
    }
}
