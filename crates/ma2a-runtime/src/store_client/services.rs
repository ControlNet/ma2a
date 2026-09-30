use ma2a_core::{
    EndpointId, RequestId, SpaceId,
    service::{ServiceAccess, ServiceError},
};
use ma2a_store::{Committed, ExposureRecord, LocalExposure, StoreError};
use tokio::sync::oneshot;

use crate::{
    error::RuntimeError,
    store::{ServiceGrant, ServiceStoreCommand, StoreClient, StoreCommand},
};

impl StoreClient {
    async fn services<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<T>) -> ServiceStoreCommand,
    ) -> Option<T> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::Services(command(reply)))
            .await
            .ok()?;
        response.await.ok()
    }

    /// Authorizes `peer` for one access to this Runtime within one complete shared Space.
    pub(crate) async fn authorize_service(
        &self,
        (local, peer): (EndpointId, EndpointId),
        access: ServiceAccess,
    ) -> Result<ServiceGrant, ServiceError> {
        self.services(|reply| ServiceStoreCommand::Authorize {
            endpoints: (local, peer),
            access,
            reply,
        })
        .await
        .unwrap_or(Err(ServiceError::Unavailable))
    }

    /// Durably records one upload staging path before its file is created.
    pub(crate) async fn record_staging(
        &self,
        (path, request_id): (String, RequestId),
        created_at_ms: u64,
    ) -> Result<(), ServiceError> {
        self.services(|reply| ServiceStoreCommand::RecordStaging {
            path,
            request_id,
            created_at_ms,
            reply,
        })
        .await
        .unwrap_or(Err(ServiceError::Unavailable))
    }

    /// Forgets one upload staging path after its file is gone or published.
    pub(crate) async fn forget_staging(&self, path: String) {
        let _forgotten = self
            .services(|reply| ServiceStoreCommand::ForgetStaging { path, reply })
            .await;
    }

    /// Removes every staging file left by a previous run.
    pub(crate) async fn sweep_staging(&self) -> Result<usize, RuntimeError> {
        self.services(ServiceStoreCommand::SweepStaging)
            .await
            .unwrap_or_else(|| Err(crate::store::channel_error(())))
    }

    /// Applies one owner-signed change of a member's service grants.
    pub(crate) async fn grant_owned_space_member(
        &self,
        request: crate::store::OwnedMemberGrant,
    ) -> crate::store::GrantResult {
        self.services(|reply| ServiceStoreCommand::Grant { request, reply })
            .await
            .unwrap_or_else(|| Err(crate::store::channel_error(())))
    }

    /// Adds or updates one local exposure inside one Space.
    pub(crate) async fn expose_service(
        &self,
        space_id: SpaceId,
        exposure: LocalExposure,
    ) -> Result<Result<Committed<()>, StoreError>, RuntimeError> {
        self.services(|reply| ServiceStoreCommand::Expose {
            space_id,
            exposure,
            reply,
        })
        .await
        .ok_or_else(|| crate::store::channel_error(()))
    }

    /// Removes one local exposure; the value says whether it existed.
    pub(crate) async fn unexpose_service(
        &self,
        space_id: SpaceId,
        exposure: LocalExposure,
    ) -> Result<Result<Committed<bool>, StoreError>, RuntimeError> {
        self.services(|reply| ServiceStoreCommand::Unexpose {
            space_id,
            exposure,
            reply,
        })
        .await
        .ok_or_else(|| crate::store::channel_error(()))
    }

    /// Lists every local exposure with the revision of the same read.
    pub(crate) async fn service_exposures(
        &self,
    ) -> Result<Committed<Vec<ExposureRecord>>, RuntimeError> {
        self.services(ServiceStoreCommand::Exposures)
            .await
            .ok_or_else(|| crate::store::channel_error(()))?
            .map_err(RuntimeError::from)
    }
}
