//! Process reads and steps on the store thread.

use ma2a_core::{EndpointId, service::ServiceError};
use ma2a_store::Repository;
use tokio::sync::oneshot;

use crate::store::{GoDispatchSignal, ProcessJob, ServiceStoreCommand, StoreClient, StoreCommand};

impl StoreClient {
    /// Runs one Process job against the repository on the store thread.
    ///
    /// A stopped store answers [`ServiceError::Unavailable`]; nothing ran then.
    pub(crate) async fn processes<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> T + Send + 'static,
    ) -> Result<T, ServiceError> {
        let (reply, response) = oneshot::channel();
        let job: ProcessJob = Box::new(move |repository| drop(reply.send(job(repository))));
        self.sender
            .send(StoreCommand::Services(ServiceStoreCommand::Process(job)))
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        response.await.map_err(|_| ServiceError::Unavailable)
    }

    /// Checks `Process` authorization and holds the Store thread until `dispatched` fires
    /// or drops, preventing a withdrawal from committing between the check and `go` dispatch.
    pub(crate) async fn authorize_process_go(
        &self,
        (local, peer): (EndpointId, EndpointId),
        dispatched: GoDispatchSignal,
    ) -> Result<(), ServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::Services(
                ServiceStoreCommand::AuthorizeProcessGo {
                    endpoints: (local, peer),
                    dispatched,
                    reply,
                },
            ))
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        response.await.map_err(|_| ServiceError::Unavailable)?
    }
}
